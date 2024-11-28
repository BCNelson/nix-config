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
      ./backup.nix
      ./forgejo.nix
      ./healthchecks.nix
      ./vaultwarden.nix
      ./kanidm.nix
      ./monitoring.nix
    ];

  age.rekey.hostPubkey = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAINoNd3NpbrmNofVDkrxbn4dSWwE0yiFlf9CCxGGA0Y32";

  age.secrets.porkbun_api_creds.rekeyFile = ../../secrets/store/porkbun_api_creds.age;

  security.acme = {
    acceptTerms = true;
    defaults = {
      email = "admin@nel.family";
      dnsProvider = "porkbun";
      environmentFile = config.age.secrets.porkbun_api_creds.path;
    };
  };

  services.nginx = {
    enable = true;
    recommendedGzipSettings = true;
    recommendedZstdSettings = true;
    recommendedTlsSettings = true;
    recommendedProxySettings = true;
    recommendedOptimisation = true;
    virtualHosts = {
      "vault.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 525M;
        '';
        locations = {
          "/" = {
            proxyWebsockets = true;
            proxyPass = "http://localhost:8080";
          };
        };
      };
    };
  };

  networking.firewall.allowedTCPPorts = [ 80 443 ];

  zramSwap.enable = true;

  networking.hostId = "9a637b7f";

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
}
