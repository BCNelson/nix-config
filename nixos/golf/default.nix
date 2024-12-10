{ config, lib, ... }:
{
  imports = [
    ../_mixins/roles/docker.nix
    ../_mixins/roles/gaming.nix
    ../_mixins/roles/tailscale.nix
    ../_mixins/roles/desktop
    ../_mixins/hardware/fingerprint.nix
  ];
  networking.firewall = {
    enable = true;
    allowedTCPPortRanges = [
      { from = 9090; to = 9100; } # local services
    ];
    allowedUDPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
    ];
    allowedTCPPorts = [ 22000 ]; # Syncthing
    allowedUDPPorts = [ 22000 21027 ]; # Syncthing
  };

  nix.settings.substituters = lib.mkBefore [ "https://nixcache.nel.family/" ];

  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;

  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/home/bcnelson/nix-config";
    reboot = false;
    refreshInterval = "6h";
    ntfy-refresh = {
      enable = true;
      topicFile = config.age.secrets.ntfy_refresh_topic.path;
    };
    user = "bcnelson";
  };

  zramSwap.enable = true;
}
