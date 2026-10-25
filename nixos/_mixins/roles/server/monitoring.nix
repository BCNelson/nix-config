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
        host = "${hostname}",
      }
      relabel_rules = loki.relabel.journal.rules
      forward_to    = [loki.write.loki.receiver]
    }
    ${dockerConfig}
  '';

  systemd.services.alloy.serviceConfig.SupplementaryGroups =
    lib.mkIf hasDocker [ "docker" ];

  # sanoid's own Nagios-style monitors judge each dataset against its
  # retention policy (hourly_warn/crit, daily_warn/crit, ... and pool
  # capacity), so these follow whatever datasets the host's sanoid config
  # lists. On a replication target the newest snapshot is the newest one
  # replicated, so snapshot age doubles as syncoid freshness. Pool health is
  # left to the ZfsPoolDegraded rule on whiskey.
  services.bcnelson.checks = lib.mkIf config.services.sanoid.enable (
    let
      sanoidMonitor = mode: ''
        rc=0
        out=$(sanoid --configdir /etc/sanoid --monitor-${mode} 2>&1) || rc=$?
        # One finding per line; drop the per-template config noise.
        grep -v '^ignored key' <<<"$out" | tr ',' '\n' || true
        # 0 OK, 1 WARN, 2 CRIT, 3 UNKNOWN
        echo "sanoid_monitor_status{mode=\"${mode}\",host=\"${hostname}\"} $rc" >> "$CHECK_METRICS"
        exit "$rc"
      '';
    in
    {
      sanoid-snapshots = {
        description = "sanoid snapshot ages are within each dataset's policy";
        runtimeInputs = [ config.services.sanoid.package ];
        script = sanoidMonitor "snapshots";
        interval = "15m";
        alert = {
          # Replicated snapshots normally arrive a little after the hour, so
          # give a late syncoid run time to catch up before alerting.
          for = "30m";
          summary = "Snapshots are missing or older than their sanoid policy";
          description = "sanoid --monitor-snapshots is reporting WARN or CRIT. On a replication target this usually means syncoid has stopped delivering; check the syncoid-* units on the sending host.";
        };
      };

      sanoid-capacity = {
        description = "ZFS pools are below sanoid's capacity thresholds";
        runtimeInputs = [ config.services.sanoid.package ];
        script = sanoidMonitor "capacity";
        interval = "1h";
        alert = {
          for = "1h";
          summary = "A ZFS pool is filling up";
          description = "sanoid --monitor-capacity is over its threshold (WARN at 80%, CRIT at 95%). ZFS slows down badly as a pool gets full.";
        };
      };
    }
  );
}
