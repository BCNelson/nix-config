{ pkgs, libx, ... }:
let
  dataDirs = import ./dataDirs.nix;
  services = import ./services { inherit libx dataDirs pkgs; };
  healthcheckUuid = libx.getSecret ./sensitive.nix "auto_update_healthCheck_uuid";
  porkbun_api_creds = libx.getSecretWithDefault ./sensitive.nix "porkbun_api" {
    api_key = "";
    secret_key = "";
  };
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
      ../_mixins/roles/figurine.nix
      ../_mixins/roles/server/monitoring.nix
      ./unbound.nix
      ./backups.nix
      ./nfs.nix
      ./nixarr.nix
    ];

  environment.systemPackages = [
    pkgs.zfs
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
        PORKBUN_API_KEY=${porkbun_api_creds.api_key}
        PORKBUN_SECRET_API_KEY=${porkbun_api_creds.secret_key}
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
      "media.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8096";
            extraConfig = ''
              proxy_set_header Range $http_range;
              proxy_set_header If-Range $http_if_range;
            '';
          };
          "~ (/jellyfin)?/socket" = {
            proxyWebsockets = true;
            proxyPass = "http://localhost:8096";
          };
        };
      };
      "audiobooks.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8080";
            extraConfig = ''
              proxy_set_header Host $host;
              proxy_set_header Upgrade $http_upgrade;
              proxy_set_header Connection "upgrade";
            '';
          };
        };
      };
      "nextcloud.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "https://localhost:8443";
            extraConfig = ''
              proxy_max_temp_file_size 2048m;
            '';
          };
        };
      };
      "todo.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:3456";
          };
        };
      };
      "recipes.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:9000";
          };
        };
      };
      "syncthing.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8384";
          };
        };
      };
      "pathfinder.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:30000";
            extraConfig = ''
              proxy_set_header Host $host;
              proxy_set_header Upgrade $http_upgrade;
              proxy_set_header Connection "upgrade";
            '';
          };
        };
      };
      "health.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8081";
          };
        };
      };
      "inventory.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:7745";
          };
        };
      };
      "nel.to" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/a" = {
            return = "301 https://inventory.h.b.nel.family$request_uri";
          };
        };
      };
      "photos.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:2283";
          };
          "~ (/immich)?/api" = {
            proxyPass = "http://localhost:2283";
          };
        };
      };
      "docs.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8000";
          };
        };
      };
      "tube.romeo.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8001";
          };
        };
      };
      "homeassistant.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://192.168.3.8:8123";
          };
          "/api/websocket" = {
            proxyWebsockets = true;
            proxyPass = "http://192.168.3.8:8123";
            extraConfig = ''
              proxy_set_header Host $host;
              proxy_set_header Upgrade $http_upgrade;
              proxy_set_header Connection "upgrade";
            '';
          };
        };
      };
      "budget.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:5006";
          };
        };
      };
    };
  };

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

  networking.firewall.allowedTCPPorts = [ 80 443 ];
}
