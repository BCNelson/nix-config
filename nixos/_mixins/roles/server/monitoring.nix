{ config, ... }: {
  services.prometheus = {
    exporters = {
      node = {
        enable = true;
        enabledCollectors = [ "systemd" ];
        port = 9100;
        openFirewall = true;
        firewallFilter = "-i tailscale0 -p tcp --dport 9100 -j ACCEPT";
      };
    };
  };


  services.promtail = {
    enable = true;
    configuration = {
      server = {
        http_listen_port = 3031;
        grpc_listen_port = 0;
      };
      positions = {
        filename = "/tmp/positions.yaml";
      };
      clients = [{
        url = "http://whiskey.b.nel.family:3100/loki/api/v1/push";
      }];
      scrape_configs = [
        {
          job_name = "journal";
          journal = {
            max_age = "12h";
            labels = {
              job = "systemd-journal";
              host = config.networking.hostName;
            };
          };
          relabel_configs = [{
            source_labels = [ "__journal__systemd_unit" ];
            target_label = "unit";
          }];
        }
        {
          job_name = "docker";
          docker_sd_configs = [{
            host = "unix:///var/run/docker.sock";
            host_networking_host = config.networking.hostName;
            refresh_interval = "10s";
          }];
          relabel_configs = [
            {
              source_labels = [ "__meta_docker_container_name" ];
              regex = "/(.*)";
              target_label = "container";
            }
            {
              source_labels = [ "__meta_docker_container_log_stream" ];
              target_label = "logstream";
            }
            {
              source_labels = [ "__meta_docker_container_label_logging_jobname" ];
              target_label = "job";
            }
          ];
        }
      ];
    };
  };

  networking.firewall.interfaces.tailscale0 = {
    allowedTCPPorts = [ 3031 ];
  };
}
