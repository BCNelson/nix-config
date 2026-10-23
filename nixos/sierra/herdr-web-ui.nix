{ config, ... }:

# Publish herdr web ui (home-manager/bcnelson/_mixins/herdr/web-ui.nix) to the
# tailnet at https://sierra-2.<tailnet>.ts.net/. The app listens on loopback
# only; Serve is the one way in, and it states each request's Tailscale login,
# which the app uses to let bcnelson's own devices in without pairing.
#
# 443 is free here, unlike romeo, where Serve on 443 intercepted every SNI
# ahead of nginx (see nixos/romeo/services/ollama.nix). Check that again before
# adding a web server on this host.
#
# Without --bg, Serve lasts as long as this process, so the endpoint is
# declarative: stopping or removing the unit takes it off the tailnet, rather
# than leaving a config behind in tailscaled's state. The app itself runs only
# while herdr does; in between, Serve answers 502.
{
  systemd.services.tailscale-serve-herdr-web-ui = {
    description = "Expose herdr web ui through Tailscale Serve";
    wantedBy = [ "multi-user.target" ];
    wants = [ "network-online.target" "tailscale-autoconnect.service" ];
    after = [ "network-online.target" "tailscaled.service" "tailscale-autoconnect.service" ];
    requires = [ "tailscaled.service" ];
    bindsTo = [ "tailscaled.service" ];
    partOf = [ "tailscaled.service" ];

    serviceConfig = {
      Type = "simple";
      ExecStart = "${config.services.tailscale.package}/bin/tailscale serve --https=443 http://127.0.0.1:7317";
      Restart = "on-failure";
      RestartSec = "5s";
    };
  };
}
