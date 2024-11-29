{ libx, pkgs, config, ... }:
let
  dataDirs = {
    level1 = "/data/level1"; # Critical
    level2 = "/data/level2"; # Important
    level3 = "/data/level3"; # High
    level4 = "/data/level4"; # Medium
    level5 = "/data/level5"; # Low
    level6 = "/data/level6"; # Replaceable
    level7 = "/data/level6"; # Ephemeral
  };
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
      ../_mixins/roles/server/nginx.nix
      ./backup.nix
      ./services/forgejo.nix
      ./services/healthchecks.nix
      ./services/vaultwarden.nix
      ./services/kanidm.nix
      ./services/monitoring.nix
    ];

  networking.firewall.allowedTCPPorts = [ 80 443 ];

  zramSwap.enable = true;

  networking.hostId = "9a637b7f";

  age.secrets.ntfy_topic.rekeyFile = ../../secrets/store/ntfy_topic.age;
  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;
  age.secrets.auto_update_healthCheck_uuid.rekeyFile = ../../secrets/store/whiskey/auto_update_healthCheck_uuid.age;

  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/config";
    reboot = true;
    refreshInterval = "5m";
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
      uuidFile = config.age.secrets.auto_update_healthCheck_uuid.path;
    };
  };
}
