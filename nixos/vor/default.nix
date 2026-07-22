{ config, ... }:{
  imports =
    [
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ../_mixins/roles/server/monitoring.nix
      ../_mixins/roles/server/nginx.nix
      ../_mixins/roles/server/recovery.nix
      ./services
      ./samba.nix
      ./backups.nix
      ./dataDirs.nix
    ];

  age.secrets.ntfy_topic.rekeyFile = ../../secrets/store/ntfy_topic.age;
  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;
  age.secrets.cadence_check_auto_update_vor.rekeyFile =
    ../../secrets/store/cadence/checks/auto-update-vor.age;

  services.bcnelson.autoUpdate = {
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
    healthCheck = {
      enable = true;
      url = "https://health.b.nel.family";
      uuidFile = config.age.secrets.cadence_check_auto_update_vor.path;
    };
  };

  networking.hostId = "d80836c3";
}
