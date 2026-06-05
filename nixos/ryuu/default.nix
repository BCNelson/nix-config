{ config, ... }:
{
  imports = [
    ../_mixins/roles/tailscale.nix
  ];

  age.secrets.cadence_check_auto_update_ryuu.rekeyFile =
    ../../secrets/store/cadence/checks/auto-update-ryuu.age;

  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/config";
    reboot = false;
    refreshInterval = "5m";
    healthCheck = {
      enable = true;
      url = "https://health.b.nel.family";
      uuidFile = config.age.secrets.cadence_check_auto_update_ryuu.path;
    };
  };

}
