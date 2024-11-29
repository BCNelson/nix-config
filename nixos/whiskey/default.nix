{ libx, config, ... }:
let
  ntfy_topic = libx.getSecretWithDefault ../sensitive.nix "ntfy_topic" "null";
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
      uuidFile = config.age.secrets.auto_update_healthCheck_uuid.path;
    };
  };
}
