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
# who owns that address (`tailscale whois`). The login is allowed only if the
# device is untagged and belongs to the same Tailscale user as sierra itself,
# i.e. one of my own devices. Tagged servers and other people's shared nodes
# are refused. The RDP password is ignored: the client must send *something*
# (any string), but the device identity is what's checked.
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
# Setup in System Settings -> Remote Desktop: enable it, tick "system user"
# (this host's account), turn on autostart. Logs:
#   journalctl --user -u app-org.kde.krdpserver -t krdp-tailscale-auth
let
  tailscale = config.services.tailscale.package;

  krdpTailscaleAuth = pkgs.writeShellApplication {
    name = "krdp-tailscale-auth";
    runtimeInputs = [ tailscale pkgs.jq pkgs.util-linux ];
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

      self_ip="$(tailscale ip -4 | head -n1)"
      if ! self="$(tailscale whois --json "$self_ip" 2>/dev/null)"; then
        log "refusing ''${PAM_USER:-?} from $rhost: can't resolve this host's own owner"
        exit 1
      fi

      # Login names, not numeric user IDs: the IDs are above 2^53, so jq
      # would compare them as doubles.
      owner="$(jq -r '.UserProfile.LoginName // empty' <<<"$self")"
      node="$(jq -r '.Node.Name // empty' <<<"$client")"
      login="$(jq -r '.UserProfile.LoginName // empty' <<<"$client")"
      tagged="$(jq -r '(.Node.Tags // []) | length > 0' <<<"$client")"

      if [ "$tagged" != "false" ]; then
        log "refusing ''${PAM_USER:-?} from $rhost ($node): tagged node"
        exit 1
      fi
      if [ -z "$owner" ] || [ "$login" != "$owner" ]; then
        log "refusing ''${PAM_USER:-?} from $rhost ($node): owned by someone else"
        exit 1
      fi

      log "allowing ''${PAM_USER:-?} from $rhost ($node)"
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

  networking.firewall.interfaces.tailscale0.allowedTCPPorts = [ 3389 ];
}
