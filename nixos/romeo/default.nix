{ pkgs, libx, config, ... }:
let
  dataDirs = config.data.dirs;
  services = import ./docker-services { inherit libx dataDirs pkgs; };
in
{
  imports =
    [
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ../_mixins/roles/server/monitoring.nix
      ../_mixins/roles/server/nginx.nix
      ./unbound.nix
      ./backups.nix
      ./nfs.nix
      ./nixarr.nix
      ./services
      ./dataDirs.nix
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

  services.nginx = {
    enable = true;
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
            proxyWebsockets = true;
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
      "photos.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
          proxy_set_header Upgrade $http_upgrade;
          proxy_set_header Connection "upgrade";
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:2283";
          };
          "~ (/immich)?/api" = {
            proxyPass = "http://localhost:2283";
            proxyWebsockets = true;
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
            proxyPass = "http://192.168.3.6:8123";
          };
          "/api/websocket" = {
            proxyWebsockets = true;
            proxyPass = "http://192.168.3.6:8123";
            extraConfig = ''
              proxy_set_header Host $host;
              proxy_set_header Upgrade $http_upgrade;
              proxy_set_header Connection "upgrade";
            '';
          };
          "/api/hassio_ingress" = {
            proxyWebsockets = true;
            proxyPass = "http://192.168.3.6:8123";
             extraConfig = ''
              proxy_set_header Host $host;
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

  age.secrets.ntfy_topic.rekeyFile = ../../secrets/store/ntfy_topic.age;
  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;
  age.secrets.auto_update_healthCheck_uuid.rekeyFile = ../../secrets/store/romeo/auto_update_healthCheck_uuid.age;

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

  networking.firewall.allowedTCPPorts = [ 80 443 8088 ];
}
