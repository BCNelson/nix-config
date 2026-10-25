{ config, lib, outputs, ... }:

# Prometheus alert rules -> Alertmanager -> alertmanager-ntfy -> ntfy.sh, on
# the same topic as the startup notifications.
#
# Two kinds of rules:
#   - core rules below, over node-exporter metrics from every scraped host;
#   - one Failing/Stale pair per custom check (modules/nixos/checks), defined
#     on the host that runs the check and collected here.

let
  hostname = config.networking.hostName;

  # Reading another host's checkAlertRules evaluates that host's config as
  # part of whiskey's, so a host that fails to evaluate fails whiskey too. CI
  # evaluates every host anyway. The ISOs are skipped: they never run checks.
  otherHosts = lib.filterAttrs
    (name: _: name != hostname && !lib.hasPrefix "iso_" name)
    outputs.nixosConfigurations;
  checkRules = lib.filterAttrs (_: rules: rules != [ ])
    (lib.mapAttrs (_: host: host.config.services.bcnelson.checkAlertRules) otherHosts
      // { ${hostname} = config.services.bcnelson.checkAlertRules; });

  scrapedHosts = lib.concatMap
    (job: map (sc: sc.labels.host or null) (job.static_configs or [ ]))
    config.services.prometheus.scrapeConfigs;

  ntfyPort = 9095;

  coreRules = [
    {
      alert = "HostDown";
      expr = ''up{host!=""} == 0'';
      for = "5m";
      labels.severity = "critical";
      annotations = {
        summary = "Host is not answering Prometheus";
        description = "node-exporter at {{ $labels.instance }} has been unreachable for 5 minutes: the host is down, off the tailnet, or node-exporter has stopped. Its checks will go stale too.";
      };
    }
    {
      alert = "FilesystemAlmostFull";
      # ZFS is left to the sanoid-capacity check: a dataset's size is its
      # usage plus the pool's free space, so per-dataset percentages mislead.
      expr = ''
        (1 - node_filesystem_avail_bytes{fstype!~"tmpfs|ramfs|squashfs|overlay|nsfs|zfs|fuse.*"}
          / node_filesystem_size_bytes) > 0.9
        and node_filesystem_readonly == 0
      '';
      for = "15m";
      labels.severity = "warning";
      annotations = {
        summary = "{{ $labels.mountpoint }} is over 90% full";
        description = "{{ $value | humanizePercentage }} used on {{ $labels.device }}.";
      };
    }
    {
      alert = "SystemdUnitFailed";
      # Checks never fail their unit; their result is reported by their own
      # rules instead.
      expr = ''node_systemd_unit_state{state="failed",name!~"check-.*"} == 1'';
      for = "15m";
      labels.severity = "warning";
      annotations = {
        summary = "{{ $labels.name }} has failed";
        description = "Logs: journalctl -u {{ $labels.name }} on {{ $labels.host }}";
      };
    }
    {
      alert = "ZfsPoolDegraded";
      expr = ''node_zfs_zpool_state{state!="online"} > 0'';
      for = "5m";
      labels.severity = "critical";
      annotations = {
        summary = "ZFS pool {{ $labels.zpool }} is {{ $labels.state }}";
        description = "Run zpool status -x on {{ $labels.host }}. A degraded pool has lost its redundancy.";
      };
    }
    {
      alert = "CheckResultsUnreadable";
      expr = "node_textfile_scrape_error == 1";
      for = "15m";
      labels.severity = "warning";
      annotations = {
        summary = "node-exporter cannot read the custom check results";
        description = "A .prom file in /var/lib/node-exporter-textfile is malformed (usually extra metrics a check wrote to $CHECK_METRICS). The affected checks will also go stale. node-exporter logs the file and the error.";
      };
    }
  ];
in
{
  assertions = map
    (host: {
      assertion = builtins.elem host scrapedHosts;
      message = "${host} defines services.bcnelson.checks but whiskey does not scrape it; add it to nodeTargets in nixos/whiskey/services/monitoring.nix.";
    })
    (lib.attrNames checkRules);

  services.prometheus = {
    rules = [
      (builtins.toJSON {
        groups = [{ name = "core"; rules = coreRules; }]
          ++ lib.mapAttrsToList (host: rules: { name = "checks-${host}"; inherit rules; }) checkRules;
      })
    ];

    alertmanagers = [{
      static_configs = [{ targets = [ "127.0.0.1:${toString config.services.prometheus.alertmanager.port}" ]; }];
    }];

    alertmanager = {
      enable = true;
      listenAddress = "127.0.0.1";
      # Single instance: don't open the HA gossip port on every interface.
      extraFlags = [ "--cluster.listen-address=" ];
      configuration = {
        route = {
          receiver = "ntfy";
          # One notification per alert per host/check, not one per rule.
          group_by = [ "alertname" "host" "check" ];
          group_wait = "30s";
          group_interval = "5m";
          repeat_interval = "12h";
        };
        receivers = [{
          name = "ntfy";
          webhook_configs = [{
            url = "http://127.0.0.1:${toString ntfyPort}/hook";
            send_resolved = true;
          }];
        }];
      };
    };

    alertmanager-ntfy = {
      enable = true;
      settings = {
        # The module's default (8000) is healthchecks.
        http.addr = "127.0.0.1:${toString ntfyPort}";
        ntfy = {
          baseurl = "https://ntfy.sh";
          notification = {
            # A topic on ntfy.sh is effectively a password, so the real one
            # comes from the secret file below.
            topic = "";
            priority = ''status == "firing" ? (labels.severity == "critical" ? "urgent" : "high") : "default"'';
            templates.title = ''
              {{ if eq .Status "resolved" }}Resolved: {{ end }}{{ with .Labels.host }}[{{ . }}] {{ end }}{{ index .Annotations "summary" }}
            '';
          };
        };
      };
      extraConfigFiles = [ config.age-template.files.alertmanager-ntfy-topic.path ];
    };
  };

  age-template.files.alertmanager-ntfy-topic = {
    vars.topic = config.age.secrets.ntfy_topic.path;
    content = ''
      ntfy:
        notification:
          topic: "$topic"
    '';
  };
}
