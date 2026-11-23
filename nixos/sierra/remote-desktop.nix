{ config, lib, pkgs, ... }:

# Remote desktop into sierra's Plasma session from the phone, over Tailscale,
# with the Tailscale identity of the connecting device as the login.
#
# The server is KRDP (kdePackages.krdp), which Plasma 6 already installs. It is
# a per-user server inside the running session: it shares the real desktop, so
# there must be a logged-in session (at the SDDM greeter there is nothing to
# connect to), and a locked screen shows up as the lock screen, which can be
# unlocked remotely.
#
# How a login is checked
# ----------------------
# With "system user" ticked in System Settings -> Remote Desktop, KRDP checks
# logins through PAM. Stock KRDP 6.8 hands PAM almost nothing: it uses the
# shared `login` service and never says where the client is connecting from
# (no PAM_RHOST), although FreeRDP has the address in peer->hostname. The
# patch (patches/krdp-pam-rhost.patch) makes it:
#   - use its own PAM service, `krdp`, so this policy doesn't touch console
#     logins;
#   - pass the client's address as PAM_RHOST;
#   - name the user in pam_start, so `account` modules know who it is.
#
# The `krdp` PAM service then runs krdp-tailscale-auth, which asks tailscaled
# about that address (`tailscale whois`) and reads the app capabilities the
# tailnet policy grants it on sierra. The login is allowed only if a grant
# gives the `nel.family/cap/krdp` capability with the RDP user (or "*") in
# its `users` list. Who may log in is therefore decided in the Tailscale
# policy (admin console), not here, e.g.:
#
#   "grants": [{
#     "src": ["autogroup:member"], "dst": ["autogroup:self"],
#     "ip":  ["tcp:3389"],
#     "app": { "nel.family/cap/krdp": [{ "users": ["bcnelson"] }] }
#   }]
#
# The same grant opens the port. With no such grant every login is refused.
# The RDP password is ignored: the client must send *something* (any string),
# but the device's grant is what's checked.
#
# KRDP only tries PAM for the username of the logged-in session (it compares
# against KUser().loginName() before calling PAM), so this can only ever open
# bcnelson's own running desktop.
#
# The PAM stack runs inside krdpserver, i.e. as bcnelson, not root. That's
# fine: `tailscale whois` is a read-only LocalAPI call any local user may make.
#
# Port 3389 is open on tailscale0 only, so a client address that is not on the
# tailnet can't reach it in the first place, and whois fails closed for one
# that somehow did.
#
# krdp-setup (a user unit, below) runs before every start of the server and
# does what System Settings -> Remote Desktop would otherwise have to: it
# creates the TLS certificate if there isn't one and turns on PAM login.
# Without it, krdpserver exits with "A valid TLS certificate ... is required",
# because only that settings page ever generates one. Turning on autostart is
# still done there. Logs:
#   journalctl --user -u krdp-setup -u app-org.kde.krdpserver -t krdp-tailscale-auth
let
  tailscale = config.services.tailscale.package;

  # The app capability a tailnet grant must give a device on sierra for it to
  # log in. The name is arbitrary, but must sit under a domain I control.
  krdpCapability = "nel.family/cap/krdp";

  krdpTailscaleAuth = pkgs.writeShellApplication {
    name = "krdp-tailscale-auth";
    runtimeInputs = [ tailscale pkgs.jq pkgs.util-linux pkgs.coreutils ];
    text = ''
      log() { logger -t krdp-tailscale-auth -- "$*"; }

      # Only gate authentication; the account phase has nothing to add.
      if [ "''${PAM_TYPE:-}" != "auth" ]; then
        exit 0
      fi

      rhost="''${PAM_RHOST:-}"
      # A dual-stack listener reports IPv4 clients as IPv4-mapped IPv6.
      rhost="''${rhost#::ffff:}"
      if [ -z "$rhost" ]; then
        log "refusing ''${PAM_USER:-?}: no PAM_RHOST (is KRDP unpatched?)"
        exit 1
      fi

      if ! client="$(tailscale whois --json "$rhost" 2>/dev/null)"; then
        log "refusing ''${PAM_USER:-?} from $rhost: not a tailnet address"
        exit 1
      fi

      user="''${PAM_USER:-}"
      node="$(jq -r '.Node.Name // "?"' <<<"$client")"
      if [ -z "$user" ]; then
        log "refusing login from $rhost ($node): no PAM_USER"
        exit 1
      fi

      # CapMap holds the app capabilities the tailnet policy grants the
      # client on this node: { "<cap>": [ <grant's app value>, ... ] }, one
      # element per matching grant. Any of them naming the user (or "*") is
      # enough.
      if jq -e --arg cap ${lib.escapeShellArg krdpCapability} --arg user "$user" '
          (.CapMap[$cap] // [])
          | any(.[]; (.users // []) | any(. == $user or . == "*"))
        ' <<<"$client" >/dev/null; then
        log "allowing $user from $rhost ($node)"
        exit 0
      fi

      log "refusing $user from $rhost ($node): no ${krdpCapability} grant for this user"
      exit 1
    '';
  };

  krdpSetup = pkgs.writeShellApplication {
    name = "krdp-setup";
    runtimeInputs = [ pkgs.openssl pkgs.kdePackages.kconfig pkgs.coreutils ];
    text = ''
      rc=krdpserverrc
      dir="''${XDG_DATA_HOME:-$HOME/.local/share}/krdpserver"
      crt="$dir/krdp.crt"
      key="$dir/krdp.key"

      # A certificate configured by hand (or one the settings page already
      # made) is left alone; only fill in the default one when none is set.
      current="$(kreadconfig6 --file "$rc" --group General --key Certificate)"
      if [ -z "$current" ] || [ ! -s "$current" ]; then
        if [ ! -s "$crt" ] || [ ! -s "$key" ]; then
          mkdir -p "$dir"
          # Same as the settings page, plus a CN. Self-signed: the phone's
          # client will ask to trust it on first connect.
          umask 077
          openssl req -nodes -new -x509 -days 3650 -batch \
            -subj "/CN=$(uname -n)" -keyout "$key" -out "$crt"
          echo "generated $crt"
        fi
        kwriteconfig6 --file "$rc" --group General --key Certificate "$crt"
        kwriteconfig6 --file "$rc" --group General --key CertificateKey "$key"
      fi

      # PAM ("system user") login, which is where the Tailscale check lives.
      # Without it, and with no KRDP users configured, the server refuses to
      # start ("No users configured for login").
      kwriteconfig6 --file "$rc" --group General --key SystemUserEnabled --type bool true
    '';
  };
in
{
  # Only kdePackages.krdp is replaced. Nothing else in sierra's closure is
  # built against it, so a nixpkgs bump that touches KRDP's inputs rebuilds
  # just this (~25s). If the patch stops applying after a Plasma bump, this
  # host's auto-update fails to build, and CI (eval only) won't catch it.
  nixpkgs.overlays = [
    (_final: prev: {
      kdePackages = prev.kdePackages.overrideScope (_kfinal: kprev: {
        krdp = kprev.krdp.overrideAttrs (old: {
          patches = (old.patches or [ ]) ++ [ ../../patches/krdp-pam-rhost.patch ];
        });
      });
    })
  ];

  # Written out in full rather than built from the module's options, which
  # would add pam_unix to `auth` and with it the password check this replaces.
  # pam_exec without expose_authtok never prompts, so the password KRDP holds
  # is never read.
  security.pam.services.krdp.text = ''
    auth    required pam_exec.so quiet ${lib.getExe krdpTailscaleAuth}
    account required pam_unix.so
  '';

  # The server unit (app-org.kde.krdpserver) ships with KRDP; this hangs off
  # it, so it runs whenever that is started, however that happens (autostart,
  # the settings page, or by hand), and a failure here stops it starting.
  systemd.user.services.krdp-setup = {
    description = "Prepare the KRDP server's certificate and login settings";
    before = [ "app-org.kde.krdpserver.service" ];
    requiredBy = [ "app-org.kde.krdpserver.service" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = lib.getExe krdpSetup;
    };
  };

  networking.firewall.interfaces.tailscale0.allowedTCPPorts = [ 3389 ];
}
