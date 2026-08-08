{ config, lib, pkgs, ... }:
let
  # Both the bridge's bind port and the Tailscale Serve listener port, and they
  # have to be the same number.
  #
  # The bridge's DNS-rebinding guard rejects any request whose Host header is
  # not loopback unless the *port in that Host header* equals its own bind port
  # (host_authority_allowed in bridge/src/web_bridge.rs). A plain
  # `tailscale serve 8787` publishes the tailnet URL on :443, so every /api and
  # /ws request would arrive with `Host: <machine>.<tailnet>.ts.net` and be
  # answered with 403. Serving on :8787 keeps the port in the Host header and
  # makes the check pass. See ../../../../nixos/_mixins/roles/herdr-web.nix.
  port = 8787;

  # Mirrors upstream's scripts/run-bridge.sh: point the bridge at the stable
  # herdr socket rather than letting it discover herdr-dev's app directory.
  socketPath = "${config.xdg.configHome}/herdr/herdr.sock";

  # A hostname Host header is only accepted if that exact name was passed to
  # --allow-host, and the name Serve answers on is the machine's MagicDNS name.
  # Ask tailscaled for it at start instead of hardcoding the tailnet, so this
  # mixin stays correct on every host that imports it.
  herdr-web-start = pkgs.writeShellApplication {
    name = "herdr-web-start";
    runtimeInputs = [ pkgs.herdr-web pkgs.jq pkgs.tailscale ];
    # `|| true` on both steps because writeShellApplication runs under
    # `set -euo pipefail`: a host without a tailnet (or with tailscaled down)
    # should still get a loopback bridge rather than a unit that refuses to
    # start.
    text = ''
      allow=()
      status="$(tailscale status --json 2>/dev/null || true)"
      if [ -n "$status" ]; then
        dnsName="$(printf '%s' "$status" | jq -r '.Self.DNSName // ""' 2>/dev/null || true)"
        # MagicDNS names come back fully qualified with a trailing dot.
        dnsName="''${dnsName%.}"
      else
        dnsName=""
      fi

      if [ -n "$dnsName" ]; then
        allow+=(--allow-host "$dnsName")
      else
        echo "herdr-web: tailscale reported no MagicDNS name; only loopback clients will be accepted until this unit restarts" >&2
      fi

      exec herdr-web --host 127.0.0.1 --port ${toString port} "''${allow[@]}"
    '';
  };
in
{
  # Browser client for herdr - https://github.com/kcosr/herdr-web
  #
  # The bridge talks to herdr over the *user's* socket, so it has to run as the
  # user rather than as a system service. It stays on loopback: it has no
  # authentication of its own, and Tailscale Serve is the only remote entry
  # point (NixOS hosts only - see the mixin referenced above).
  home.packages = [ pkgs.herdr-web ];

  # The bridge refuses to start unless a herdr daemon is already up and
  # reporting a compatible version/protocol (startup_daemon_status), and herdr
  # itself is started by hand from a terminal. A path unit is what lets the
  # bridge come up on its own the first time a herdr session exists, instead of
  # a service that fails on every boot until someone runs `herdr`.
  systemd.user.paths.herdr-web = {
    Unit.Description = "Watch for a herdr daemon socket";
    Path.PathExists = socketPath;
    Install.WantedBy = [ "default.target" ];
  };

  systemd.user.services.herdr-web = {
    Unit = {
      Description = "herdr-web bridge";
      Documentation = "https://github.com/kcosr/herdr-web";
      After = [ "network-online.target" ];
    };

    Service = {
      Type = "simple";
      Environment = [ "HERDR_SOCKET_PATH=${socketPath}" ];
      ExecStart = lib.getExe herdr-web-start;
      # Covers herdr being restarted underneath the bridge, and the window where
      # the socket exists but tailscaled has not settled yet.
      Restart = "on-failure";
      RestartSec = 10;
    };

    # No Install section on purpose: the path unit above owns starting this.
  };
}
