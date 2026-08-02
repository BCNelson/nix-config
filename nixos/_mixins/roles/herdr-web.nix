{ pkgs, ... }:
{
  # Publish the user's herdr-web bridge on the tailnet.
  #
  # The bridge itself is a home-manager user service bound to 127.0.0.1:8787 -
  # see ../../../home-manager/bcnelson/_mixins/herdr/web.nix, which is also
  # where the port choice is explained. It has no authentication of its own, so
  # Tailscale Serve is deliberately the only remote entry point; it must never
  # be bound to a routable interface or exposed with Funnel.
  #
  # --https=8787 rather than Serve's default 443: the bridge rejects any
  # non-loopback request whose Host header port is not its own bind port, so the
  # tailnet URL has to keep :8787. Reachable at
  # https://<machine>.<tailnet>.ts.net:8787.
  #
  # Requires ./tailscale.nix on the same host.
  systemd.services.tailscale-herdr-web-serve = {
    description = "Expose herdr-web through Tailscale Serve";
    wantedBy = [ "multi-user.target" ];
    wants = [ "network-online.target" "tailscale-autoconnect.service" ];
    after = [
      "network-online.target"
      "tailscaled.service"
      "tailscale-autoconnect.service"
    ];
    requires = [ "tailscaled.service" ];
    bindsTo = [ "tailscaled.service" ];
    partOf = [ "tailscaled.service" ];

    serviceConfig = {
      Type = "simple";
      # Without --bg, `tailscale serve` owns the proxy for this process
      # lifetime, so stopping this unit or Tailscale also removes the Tailnet
      # endpoint. Same shape as romeo's ollama serve unit.
      #
      # Serve does not care whether the backend is listening, so this unit is
      # not tied to the user's bridge: it publishes the endpoint and the bridge
      # comes and goes behind it.
      ExecStart = "${pkgs.unstable.tailscale}/bin/tailscale serve --https=8787 http://127.0.0.1:8787";
      Restart = "on-failure";
      RestartSec = "5s";
    };
  };
}
