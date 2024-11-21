{ libx, pkgs, ... }:
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
  services = import ./services { inherit libx dataDirs pkgs; };
  healthcheckUuid = libx.getSecretWithDefault ./sensitive.nix "auto_update_healthCheck_uuid" "00000000-0000-0000-0000-000000000000";
  porkbun_api_key = libx.getSecret ./sensitive.nix "porkbun_api_key";
  porkbun_api_secret = libx.getSecret ./sensitive.nix "porkbun_api_secret";
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
      ./kanidm.nix
      ./monitoring.nix
    ];
  environment.systemPackages = [
    services.networkBacked
  ];

  systemd.timers.auto-update-services = {
    enable = true;
    timerConfig = {
      OnBootSec = "30min";
      OnUnitActiveSec = "60m";
      Persistent = true;
    };
    wantedBy = [ "timers.target" ];
  };

  systemd.services.auto-update-services = {
    enable = true;
    path = [ pkgs.docker ];
    serviceConfig = {
      Type = "oneshot";
      User = "root";
      ExecStart = "${services.networkBacked}/bin/dockerStack-general up -d --remove-orphans --pull always --quiet-pull";
    };
    restartTriggers = [ services.networkBacked ];
    restartIfChanged = false;
  };

  security.acme = {
    acceptTerms = true;
    defaults = {
      email = "admin@nel.family";
      dnsProvider = "porkbun";
      environmentFile = "${pkgs.writeText "porkbun-creds" ''
        PORKBUN_SECRET_API_KEY=${porkbun_api_secret}
        PORKBUN_API_KEY=${porkbun_api_key}
      ''}";
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
      "health.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        locations = {
          "/" = {
            proxyPass = "http://localhost:8000";
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
