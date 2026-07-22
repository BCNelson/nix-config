{ pkgs, libx, config, ... }:
let
  dataDirs = config.data.dirs;
  services = import ./docker-services { inherit libx dataDirs pkgs; };
  cadencePingStartExec = slug: pkgs.writeShellScript "cadence-ping-${slug}-start" ''
    ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 \
      "https://health.b.nel.family/ping/$(cat /run/agenix/cadence_check_${builtins.replaceStrings [ "-" ] [ "_" ] slug})/start" \
      || true
  '';
  cadencePingResultExec = slug: pkgs.writeShellScript "cadence-ping-${slug}-result" ''
    url="https://health.b.nel.family/ping/$(cat /run/agenix/cadence_check_${builtins.replaceStrings [ "-" ] [ "_" ] slug})"
    if [ "$SERVICE_RESULT" != "success" ]; then url="$url/fail"; fi
    # Post this invocation's journal as the ping body so cadence captures
    # failure context. 10 KiB cap on cadence side; leave headroom.
    ${pkgs.systemd}/bin/journalctl _SYSTEMD_INVOCATION_ID="$INVOCATION_ID" \
        --no-pager --no-hostname -o short-iso 2>/dev/null \
      | tail -n 200 | tail -c 9000 \
      | ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 \
          -H "Content-Type: text/plain; charset=utf-8" \
          --data-binary @- \
          "$url" || true
  '';
in
{
  imports =
    [
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ../_mixins/roles/server/monitoring.nix
      ../_mixins/roles/server/nginx.nix
      ../_mixins/roles/server/recovery.nix
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
      ExecStartPre = "${cadencePingStartExec "auto-update-services-romeo"}";
      ExecStart = "${services.networkBacked}/bin/dockerStack-general up -d --remove-orphans --pull always --quiet-pull";
      ExecStopPost = "${cadencePingResultExec "auto-update-services-romeo"}";
    };
    restartTriggers = [ services.networkBacked ];
    restartIfChanged = false;
  };

  systemd.services.docker-compose-shutdown = {
    description = "Stop docker-compose stacks on shutdown";
    wantedBy = [ "multi-user.target" ];
    after = [ "docker.service" ];
    requires = [ "docker.service" ];
    path = [ pkgs.docker ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      ExecStart = "${pkgs.coreutils}/bin/true";
      ExecStop = "${services.networkBacked}/bin/dockerStack-general down -t 20";
      TimeoutStopSec = 30;
    };
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
              proxy_set_header Upgrade $http_upgrade;
              proxy_set_header Connection "upgrade";
            '';
          };
          "/api/hassio_ingress" = {
            proxyWebsockets = true;
            proxyPass = "http://192.168.3.6:8123";
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
      # Reverse-proxy *.cwnel.com (Carter's portfolio apps) to bulbasaur's k3s
      # Traefik ingress. romeo terminates TLS here with a wildcard cert; Traefik
      # routes by Host. recommendedProxySettings forwards Host + X-Forwarded-For
      # so the apps see the real client IP.
      #
      # Reached over the shared physical LAN (bulbasaur is 192.168.3.82), NOT
      # bulbasaur's Tailscale IP: bulbasaur is shared into our tailnet from
      # Carter's, and Tailscale node-sharing is user-identity based, so romeo
      # (a tagged device, tag:server) cannot reach a shared node over Tailscale
      # at all. The LAN path sidesteps that entirely. See tailscale#5321.
      "*.cwnel.com" = {
        useACMEHost = "cwnel.com";
        forceSSL = true;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://192.168.3.82:80";
            proxyWebsockets = true;
          };
        };
      };
    };
  };

  # *.cwnel.com lives on Cloudflare, so this cert overrides the host default
  # (Porkbun) with a Cloudflare DNS-01 challenge. Pattern mirrors
  # nixos/whiskey/services/forgejo.nix.
  age.secrets.cwnel_cloudflare_dns_api_token.rekeyFile = ../../secrets/store/romeo/cwnel_cloudflare_dns_api_token.age;
  age-template.files."cwnel-cloudflare-acme-env" = {
    vars.token = config.age.secrets.cwnel_cloudflare_dns_api_token.path;
    content = "CF_DNS_API_TOKEN=$token";
  };
  security.acme.certs."cwnel.com" = {
    domain = "*.cwnel.com";
    dnsProvider = "cloudflare";
    # romeo's local unbound has a "cwnel.com" redirect local-zone (hairpin for
    # Carter's portfolio apps) that synthesizes empty NODATA answers with no SOA
    # for *.cwnel.com names. That breaks lego's zone auto-detection: it walks up
    # to "com." and asks Cloudflare for a nonexistent "com" zone. Point lego at a
    # public resolver so zone detection and propagation checks bypass unbound.
    dnsResolver = "1.1.1.1:53";
    environmentFile = config.age-template.files."cwnel-cloudflare-acme-env".path;
    group = "nginx";
  };

  age.secrets.ntfy_topic.rekeyFile = ../../secrets/store/ntfy_topic.age;
  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;
  age.secrets.cadence_check_auto_update_romeo.rekeyFile =
    ../../secrets/store/cadence/checks/auto-update-romeo.age;
  age.secrets.cadence_check_auto_update_services_romeo.rekeyFile =
    ../../secrets/store/cadence/checks/auto-update-services-romeo.age;

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
      uuidFile = config.age.secrets.cadence_check_auto_update_romeo.path;
    };
  };

  networking.firewall.allowedTCPPorts = [ 80 443 8088 ];
}
