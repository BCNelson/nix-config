{ config, modulesPath, ... }:
{
  imports = [
    (modulesPath + "/profiles/minimal.nix")
    ../_mixins/roles/tailscale.nix
    ./dataDirs.nix
  ];

  age.secrets.ntfy_topic.rekeyFile = ../../secrets/store/ntfy_topic.age;
  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;

  services.bcnelson = {
    autoUpdate = {
      enable = true;
      path = "/config";
      reboot = true;
      refreshInterval = "1h";
      ntfy = {
        enable = true;
        topicFile = config.age.secrets.ntfy_topic.path;
      };
      ntfy-refresh = {
        enable = true;
        topicFile = config.age.secrets.ntfy_refresh_topic.path;
      };
    };
    sign = {
      enable = true;
      urls = [ "https://homeassistant.h.b.nel.family/kitchen-dashboard/0" ];
    };
  };

}
