{ config, lib, ... }:
let
  hasDocker = config.virtualisation.docker.enable;
  hostname = config.networking.hostName;

  dockerConfig = lib.optionalString hasDocker ''

    discovery.docker "containers" {
      host = "unix:///var/run/docker.sock"
    }

    discovery.relabel "docker" {
      targets = discovery.docker.containers.targets

      rule {
        source_labels = ["__meta_docker_container_name"]
        regex         = "/(.*)"
        target_label  = "container"
      }

      rule {
        source_labels = ["__meta_docker_container_log_stream"]
        target_label  = "logstream"
      }

      rule {
        source_labels = ["__meta_docker_container_label_logging_jobname"]
        target_label  = "job"
      }
    }

    loki.source.docker "containers" {
      host    = "unix:///var/run/docker.sock"
      targets = discovery.relabel.docker.output

      forward_to = [loki.write.loki.receiver]
    }
  '';
in
{
  services.prometheus = {
    exporters = {
      node = {
        enable = true;
        enabledCollectors = [ "systemd" ];
        port = 9100;
        openFirewall = true;
        firewallFilter = "-i tailscale0 -p tcp --dport 9100";
      };
    };
  };

  services.alloy.enable = true;

  environment.etc."alloy/config.alloy".text = ''
    loki.write "loki" {
      endpoint {
        url = "http://whiskey.b.nel.family:3100/loki/api/v1/push"
      }
    }

    loki.relabel "journal" {
      forward_to = [loki.write.loki.receiver]

      rule {
        source_labels = ["__journal__systemd_unit"]
        target_label  = "unit"
      }
    }

    loki.source.journal "journal" {
      max_age     = "12h"
      labels      = {
        job  = "systemd-journal",
        host = "${hostname}",
      }
      relabel_rules = loki.relabel.journal.rules
      forward_to    = [loki.relabel.journal.receiver]
    }
    ${dockerConfig}
  '';

  systemd.services.alloy.serviceConfig.SupplementaryGroups =
    lib.mkIf hasDocker [ "docker" ];
}
