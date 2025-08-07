{ ... }:
{
  imports = [
    ../_mixins/roles/tailscale.nix
    ../_mixins/roles/docker.nix
  ];

  services.bcnelson = {
    autoUpdate = {
      enable = true;
      path = "/config";
      reboot = false;
      refreshInterval = "5m";
    };
    sign = {
      enable = true;
      urls = [ "https://homeassistant.h.b.nel.family" ];
    };
  };

}
