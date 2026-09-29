{ config, ... }: {
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

      # Under /var/lib (the module's StateDirectory). This used to be /tmp/loki,
      # which systemd's PrivateTmp wiped on every restart - about a day of logs
      # was all that ever survived a deploy.
      common = {
        path_prefix = config.services.loki.dataDir;
        storage.filesystem = {
          chunks_directory = "${config.services.loki.dataDir}/chunks";
          rules_directory = "${config.services.loki.dataDir}/rules";
        };
        replication_factor = 1;
        ring = {
          kvstore = {
            store = "inmemory";
          };
          instance_addr = "127.0.0.1";
        };
      };

      # ~200 MB/day of journal across the hosts, so 30 days is ~6 GB on a root
      # disk with ~48 GB free. Retention is enforced by the compactor.
      limits_config.retention_period = "720h";

      compactor = {
        working_directory = "${config.services.loki.dataDir}/compactor";
        retention_enabled = true;
        delete_request_store = "filesystem";
      };

      schema_config = {
        configs = [
          {
            from = "2020-09-07";
            store = "tsdb";
            object_store = "filesystem";
            schema = "v13";
            index = {
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

  services.alloy.enable = true;

  environment.etc."alloy/config.alloy".text = ''
    loki.write "loki" {
      endpoint {
        url = "http://127.0.0.1:3100/loki/api/v1/push"
      }
    }

    // Only a rule set for loki.source.journal below, which is where the
    // __journal_* fields still exist. Routing entries through this component
    // as well re-ran the rule without them and blanked `unit` again.
    loki.relabel "journal" {
      forward_to = []

      rule {
        source_labels = ["__journal__systemd_unit"]
        target_label  = "unit"
      }
    }

    loki.source.journal "journal" {
      max_age     = "12h"
      labels      = {
        job  = "systemd-journal",
        host = "${config.networking.hostName}",
      }
      relabel_rules = loki.relabel.journal.rules
      forward_to    = [loki.write.loki.receiver]
    }
  '';
}
