{ pkgs, config, lib, ... }:
{
  imports = [
    ../_mixins/roles/tailscale.nix
  ];

  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;

  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/config";
    reboot = false;
    refreshInterval = "6h";
    ntfy-refresh = {
      enable = true;
      topicFile = config.age.secrets.ntfy_refresh_topic.path;
    };
  };

  services.displayManager.sddm.settings = {
    Users = {
      HideUsers = "bcnelson";
    };
  };

  environment.plasma6.excludePackages = with pkgs.kdePackages; [ 
    plasma-browser-integration
    konsole
    (lib.getBin qttools) # Expose qdbus in PATH
    ark
    elisa
    gwenview
    okular
    kate
    khelpcenter
    dolphin
    baloo-widgets # baloo information in Dolphin
    dolphin-plugins
    spectacle
    ffmpegthumbs
    krdp
  ];

  services.xserver.excludePackages = [ pkgs.xterm ];
}
