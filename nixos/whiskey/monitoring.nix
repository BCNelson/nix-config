{ config, ... }: {
  # grafana configuration
  services.grafana = {
    enable = true;
    settings.server = {
      root_url = "https://grafana.b.nel.family";
      enable_gzip = true;
      enforce_domain = true;
      domain = "grafana.b.nel.family";
      http_port = 2342;
      http_addr = "127.0.0.1";
    };
  };

  # nginx reverse proxy
  services.nginx.virtualHosts.${config.services.grafana.domain} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString config.services.grafana.settings.server.http_port}";
      proxyWebsockets = true;
    };
  };

  services.prometheus = {
    enable = true;
    port = 9001;
    exporters = {
      node = {
        enable = true;
        enabledCollectors = [ "systemd" ];
        port = 9100;
      };
    };
    scrapeConfigs = [
      {
        job_name = "whiskey";
        static_configs = [
          {
            targets = [ "127.0.0.1:9100" ];
          }
        ];
      }
      {
        job_name = "romeo";
        static_configs = [
          {
            targets = [ "romeo.b.nel.family:9100" ];
          }
        ];
      }
      {
        job_name = "vor";
        static_configs = [
          {
            targets = [ "vor.ck.nel.family:9100" ];
          }
        ];
      }
      {
        job_name = "homeassistant";
        static_configs = [
          {
            targets = [ "homeassistant.b.nel.family:9100" ];
          }
        ];
      }
    ];
  };

  services.loki = {
    enable = true;
    configuration = {
      server.http_listen_port = 3100;
      auth_enabled = false;

      common = {
        path_prefix = "/tmp/loki";
        storage.filesystem = {
          chunks_directory = "/tmp/loki/chunks";
          rules_directory = "/tmp/loki/rules";
        };
        replication_factor = 1;
        ring = {
          kvstore = {
            store = "inmemory";
          };
          instance_addr = "127.0.0.1";
        };
      };

      schema_config = {
        configs = [
          {
            from = "2020-09-07";
            store = "boltdb-shipper";
            object_store = "filesystem";
            schema = "v12";
            index = {
              prefix = "loki_index_";
              period = "24h";
            };
          }
        ];
      };
    };
  };

  networking.firewall.interfaces.tailscale0 = {
    allowedTCPPorts = [ 3100 ];
  };
}
