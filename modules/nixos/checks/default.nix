{ config, lib, pkgs, ... }:

# Custom checks: a script run on a timer whose result node-exporter's textfile
# collector exposes to Prometheus, plus one pair of alert rules per check.
#
# The rules are only *generated* here, from the host the check runs on; they
# are evaluated by Prometheus on whiskey, which collects
# services.bcnelson.checkAlertRules from every host in the flake
# (nixos/whiskey/services/alerting.nix). Keeping the definition on the host
# means a check lives next to the service it watches.

let
  cfg = config.services.bcnelson.checks;
  hostname = config.networking.hostName;

  # node-exporter reads every *.prom file here on each scrape. Each check owns
  # check-<name>.prom and replaces it atomically, so a scrape never sees a
  # half-written file.
  textfileDir = "/var/lib/node-exporter-textfile";

  # One syntax that systemd timers, coreutils timeout and Prometheus `for`
  # all accept.
  durationType = lib.types.strMatching "[0-9]+[smhd]";
  toSeconds = d:
    let m = builtins.match "([0-9]+)([smhd])" d;
    in lib.toInt (builtins.elemAt m 0) * { s = 1; m = 60; h = 3600; d = 86400; }.${builtins.elemAt m 1};

  # frigate-detector -> FrigateDetector, for alert names.
  alertBase = name: lib.concatMapStrings
    (w: lib.toUpper (lib.substring 0 1 w) + lib.substring 1 (-1) w)
    (lib.splitString "-" name);

  selector = name: ''{check="${name}",host="${hostname}"}'';

  mkRunner = name: check:
    let
      script = pkgs.writeShellApplication {
        name = "check-${name}";
        inherit (check) runtimeInputs;
        text = check.script;
      };
      labels = ''check="${name}",host="${hostname}"'';
    in
    pkgs.writeShellScript "run-check-${name}" ''
      set -u
      # Prepend: the check inherits this PATH, and systemd's default one
      # (grep, sed, findutils, ...) is what scripts can assume beyond
      # runtimeInputs.
      PATH=${lib.makeBinPath [ pkgs.coreutils pkgs.gawk ]}:$PATH

      extra=$(mktemp)
      tmp=
      trap 'rm -f "$extra" $tmp' EXIT

      # The check never fails the unit: its result is the metric, and a failed
      # unit would also trip SystemdUnitFailed for the same problem. The
      # script's own output stays in the journal for the alert to point at.
      start=$EPOCHREALTIME
      CHECK_METRICS=$extra timeout ${toString (toSeconds check.timeout)} ${lib.getExe script}
      rc=$?
      end=$EPOCHREALTIME

      if [ "$rc" -eq 0 ]; then
        success=1
      elif [ "$rc" -eq 124 ]; then
        success=0
        echo "check timed out after ${check.timeout}" >&2
      else
        success=0
        echo "check failed (exit $rc)" >&2
      fi
      duration=$(awk -v s="$start" -v e="$end" 'BEGIN { print e - s }')

      # Every file repeats the same HELP/TYPE: the textfile collector rejects
      # a metric family whose help text differs between files.
      tmp=$(mktemp -p ${textfileDir} .check-${name}.XXXXXX)
      {
        echo "# HELP check_success Whether the last run of a custom check passed (1) or failed (0)."
        echo "# TYPE check_success gauge"
        echo 'check_success{${labels}}' "$success"
        echo "# HELP check_duration_seconds How long the last run of a custom check took."
        echo "# TYPE check_duration_seconds gauge"
        echo 'check_duration_seconds{${labels}}' "$duration"
        echo "# HELP check_last_run_timestamp_seconds When a custom check last finished."
        echo "# TYPE check_last_run_timestamp_seconds gauge"
        echo 'check_last_run_timestamp_seconds{${labels}}' "$(date +%s)"
        cat "$extra"
      } > "$tmp"
      chmod 0644 "$tmp"
      mv -f "$tmp" ${textfileDir}/check-${name}.prom
    '';

  mkRules = name: check:
    let
      sel = selector name;
      base = alertBase name;
      labels = {
        inherit (check.alert) severity;
        check = name;
        host = hostname;
      };
      # A templated severity only resolves against the Failing expression's
      # labels; the Broken and Stale rules have none to offer.
      plainLabels = labels // lib.optionalAttrs (lib.hasInfix "{{" check.alert.severity) { severity = "warning"; };
      logs = "Logs: journalctl -u check-${name} on ${hostname}";
      staleAfter = if check.alert.staleAfter != null then check.alert.staleAfter else "${toString (3 * toSeconds check.interval)}s";
      scriptFailed = "check_success${sel} == 0";
    in
    [
      {
        alert = "${base}Failing";
        expr = if check.alert.expr == null then scriptFailed else check.alert.expr;
        inherit (check.alert) for;
        inherit labels;
        annotations = {
          inherit (check.alert) summary;
          description = lib.concatStringsSep "\n\n" (lib.filter (s: s != "") [ check.alert.description logs ]);
        };
      }
    ]
    # With a custom expression the script's exit status no longer is the
    # result, only whether it managed to produce one.
    ++ lib.optional (check.alert.expr != null) {
      alert = "${base}Broken";
      expr = scriptFailed;
      for = "15m";
      labels = plainLabels;
      annotations = {
        summary = "Check ${name} cannot run";
        description = ''
          check-${name} is exiting non-zero, so its metrics, and with them "${check.description}", are not being checked.

          ${logs}'';
      };
    }
    ++ [
      {
        alert = "${base}Stale";
        # absent() covers a check that has never reported (or whose host is
        # gone); the age test covers one that stopped.
        expr = "absent(check_last_run_timestamp_seconds${sel}) or (time() - check_last_run_timestamp_seconds${sel}) > ${toString (toSeconds staleAfter)}";
        for = "5m";
        labels = plainLabels;
        annotations = {
          summary = "Check ${name} has stopped reporting";
          description = ''
            No result from check-${name} for over ${staleAfter}, so "${check.description}" is not being checked. The timer may have stopped, node-exporter may be down, or the .prom file may be malformed (node_textfile_scrape_error).

            ${logs}'';
        };
      }
    ];
in
{
  options.services.bcnelson = {
    checks = lib.mkOption {
      default = { };
      description = ''
        Custom checks. Each runs `script` every `interval`; exiting non-zero
        (or running past `timeout`) is a failure. Each check gets its own
        <Name>Failing and <Name>Stale alert rules.

        The script can also write extra Prometheus metrics, in text exposition
        format, to the file named by $CHECK_METRICS, both for graphing and for
        a custom `alert.expr`. They need no host label: the scrape adds it.

        With `alert.expr` set, Failing fires once per series the expression
        returns (say, one per dataset), and summary, description and severity
        can use Prometheus templates over that series ({{ $labels.x }},
        {{ $value }}). The script's exit status then only says whether it ran,
        and gets its own <Name>Broken rule.
      '';
      type = lib.types.attrsOf (lib.types.submodule {
        options = {
          description = lib.mkOption {
            type = lib.types.str;
            description = "What the check verifies; used for the unit description.";
          };
          script = lib.mkOption {
            type = lib.types.lines;
            description = "Bash run as root under `set -euo pipefail` (and shellcheck at build time).";
          };
          runtimeInputs = lib.mkOption {
            type = lib.types.listOf lib.types.package;
            default = [ ];
            description = "Packages on the script's PATH.";
          };
          interval = lib.mkOption {
            type = durationType;
            default = "5m";
            description = "How often to run the check.";
          };
          timeout = lib.mkOption {
            type = durationType;
            default = "1m";
            description = "Kill the script and count the run as a failure after this long.";
          };
          alert = {
            severity = lib.mkOption {
              type = lib.types.str;
              default = "warning";
              description = ''
                "critical" is pushed to ntfy as urgent, "warning" as high. May be
                a template over `alert.expr`'s labels that yields one of them.
              '';
            };
            for = lib.mkOption {
              type = durationType;
              default = "5m";
              description = "How long the check must be failing before it alerts.";
            };
            summary = lib.mkOption {
              type = lib.types.str;
              description = "One line saying what is wrong; the ntfy title.";
            };
            description = lib.mkOption {
              type = lib.types.str;
              default = "";
              description = "What the failure means or what to look at; the ntfy body.";
            };
            expr = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              description = "PromQL for the Failing rule in place of the script's exit status, usually over metrics the script wrote to $CHECK_METRICS.";
            };
            staleAfter = lib.mkOption {
              type = lib.types.nullOr durationType;
              default = null;
              description = "Alert if the check has not reported for this long. Defaults to 3x interval.";
            };
          };
        };
      });
    };

    checkAlertRules = lib.mkOption {
      type = lib.types.listOf lib.types.attrs;
      readOnly = true;
      description = "Prometheus alert rules generated from this host's checks; collected by whiskey.";
    };
  };

  config = lib.mkMerge [
    {
      services.bcnelson.checkAlertRules = lib.concatLists (lib.mapAttrsToList mkRules cfg);
    }

    (lib.mkIf (cfg != { }) {
      assertions = [
        {
          assertion = config.services.prometheus.exporters.node.enable;
          message = "services.bcnelson.checks needs node-exporter (import roles/server/monitoring.nix).";
        }
      ] ++ lib.mapAttrsToList
        (name: _: {
          assertion = builtins.match "[a-z0-9]+(-[a-z0-9]+)*" name != null;
          message = "services.bcnelson.checks.${name}: names must be lowercase kebab-case.";
        })
        cfg;

      services.prometheus.exporters.node.extraFlags = [ "--collector.textfile.directory=${textfileDir}" ];

      systemd.tmpfiles.rules = [ "d ${textfileDir} 0755 root root -" ];

      # A removed check's file would otherwise keep reporting its last result
      # forever. Only check-*.prom is ours to prune.
      system.activationScripts.prune-check-textfiles = ''
        for f in ${textfileDir}/check-*.prom; do
          [ -e "$f" ] || continue
          n=''${f##*/}
          case "''${n%.prom}" in
            ${lib.concatMapStringsSep "|" (n: "check-${n}") (lib.attrNames cfg)}) ;;
            *) rm -f "$f" ;;
          esac
        done
      '';

      systemd.services = lib.mapAttrs'
        (name: check: lib.nameValuePair "check-${name}" {
          description = "Custom check: ${check.description}";
          wants = [ "network-online.target" ];
          after = [ "network-online.target" ];
          serviceConfig = {
            Type = "oneshot";
            ExecStart = mkRunner name check;
            TimeoutStartSec = toSeconds check.timeout + 30;
          };
        })
        cfg;

      systemd.timers = lib.mapAttrs'
        (name: check: lib.nameValuePair "check-${name}" {
          wantedBy = [ "timers.target" ];
          timerConfig = {
            OnBootSec = "2m";
            OnUnitActiveSec = check.interval;
          };
        })
        cfg;
    })
  ];
}
