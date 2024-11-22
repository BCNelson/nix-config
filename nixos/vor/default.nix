{ libx, ... }:
let
  healthcheckUuid = libx.getSecretWithDefault ./sensitive.nix "auto_update_healthCheck_uuid" "00000000-0000-0000-0000-000000000000";
  ntfy_topic = libx.getSecretWithDefault ../sensitive.nix "ntfy_topic" "null";
  ntfy_autoUpdate_topic = libx.getSecretWithDefault ../sensitive.nix "ntfy_autoUpdate_topic" "null";
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ../_mixins/roles/server/monitoring.nix
      ./samba.nix
      ./backups.nix
    ];
  
  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/config";
    reboot = true;
    refreshInterval = "5m";
    ntfy = {
      enable = true;
      topic = ntfy_topic;
    };
    ntfy-refresh = {
      enable = true;
      topic = ntfy_autoUpdate_topic;
    };
    healthCheck = {
      enable = true;
      url = "https://health.b.nel.family";
      uuid = healthcheckUuid;
    };
  };

  networking.hostId = "d80836c3";
}
