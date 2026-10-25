{ config, lib, pkgs, ... }:
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
  #
  # sanoid-monitor.sh turns each finding into its own series, so each dataset
  # or pool is its own alert carrying the numbers.
  services.bcnelson.checks = lib.mkIf config.services.sanoid.enable (
    let
      sanoidMonitor = pkgs.writeShellApplication {
        name = "sanoid-monitor";
        runtimeInputs = [
          config.services.sanoid.package
          config.boot.zfs.package
          pkgs.procps
          pkgs.gawk
        ];
        text = builtins.readFile ./sanoid-monitor.sh;
      };
      sel = ''host="${hostname}"'';

      # "; receiving for 14h, 122GiB of ~1.1TiB, ~6h to go", when a zfs
      # receive into the alert's dataset is running. Each part is left out
      # when its metric is missing: bytes come from the resume token, the
      # total from the sending host's estimate (joined on the snapshot name,
      # so it works across hosts), and the time left from the receive rate
      # over the last hour.
      recvSel = ''printf "host=%q,dataset=%q" $labels.host $labels.dataset'';
      transfer = lib.concatStrings [
        "{{ $sel := ${recvSel} }}"
        ''{{ with printf "zfs_receive_running_seconds{%s}" $sel | query }}{{ with first . }}''
        "; receiving for {{ value . | humanizeDuration }}"
        ''{{ $snap := label "snapshot" . }}''
        ''{{ with printf "zfs_receive_bytes{%s}" $sel | query }}{{ with first . }}, {{ value . | humanize1024 }}B{{ end }}{{ end }}''
        "{{ if $snap }}"
        ''{{ with printf "zfs_send_estimated_bytes{snapshot=%q}" $snap | query }}{{ with first . }} of ~{{ value . | humanize1024 }}B{{ end }}{{ end }}''
        ''{{ with printf "(vector((scalar(zfs_send_estimated_bytes{snapshot=%q}) - scalar(zfs_receive_bytes{%s})) / scalar(deriv(zfs_receive_bytes{%s}[1h]))) > 0) < 31536000" $snap $sel $sel | query }}{{ with first . }}, ~{{ value . | humanizeDuration }} to go{{ end }}{{ end }}''
        "{{ end }}"
        "{{ end }}{{ end }}"
      ];
    in
    {
      sanoid-snapshots = {
        description = "sanoid snapshot ages are within each dataset's policy";
        runtimeInputs = [ sanoidMonitor ];
        script = "sanoid-monitor snapshots";
        # Often enough that a transfer's progress in the alert is current.
        interval = "5m";
        alert = {
          # Missing snapshots come out as -1 so one rule covers both kinds.
          expr = "sanoid_snapshot_age_seconds{${sel}} or (sanoid_snapshot_missing{${sel}} * -1)";
          severity = "{{ $labels.level }}";
          # Replicated snapshots normally arrive a little after the hour, so
          # give a late syncoid run time to catch up before alerting.
          for = "30m";
          summary = "{{ $labels.dataset }}: {{ if lt $value 0.0 }}no {{ $labels.type }} snapshots at all{{ else }}newest {{ $labels.type }} snapshot is {{ $value | humanizeDuration }} old (limit {{ $labels.limit }}){{ end }}${transfer}";
          description = "{{ if lt $value 0.0 }}sanoid's policy keeps {{ $labels.type }} snapshots of {{ $labels.dataset }}, but there are none.{{ else }}The newest {{ $labels.type }} snapshot of {{ $labels.dataset }} is {{ $value | humanizeDuration }} old; the {{ $labels.level }} limit is {{ $labels.limit }}.{{ end }} If a receive is in progress (see the title) this clears once it lands. Otherwise syncoid is not delivering: check the syncoid-* units on the sending host, and sanoid itself on the source.";
        };
      };

      sanoid-capacity = {
        description = "ZFS pools are below sanoid's capacity thresholds";
        runtimeInputs = [ sanoidMonitor ];
        script = "sanoid-monitor capacity";
        interval = "1h";
        alert = {
          expr = ''sanoid_pool_capacity_percent{${sel},status!="ok"}'';
          severity = "{{ $labels.status }}";
          for = "1h";
          summary = "ZFS pool {{ $labels.pool }} is {{ $value }}% full";
          description = "Over sanoid's {{ $labels.status }} threshold (warning at 80%, critical at 95%). ZFS slows down badly as a pool fills, and snapshots pin space that deleting files won't free.";
        };
      };
    }
  );
}
