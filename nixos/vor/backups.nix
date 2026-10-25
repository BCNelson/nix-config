{ lib, pkgs, ... }:
let
  cadencePingExec = slug: pkgs.writeShellScript "cadence-ping-${slug}" ''
    exec ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 \
      "https://health.b.nel.family/ping/$(cat /run/agenix/cadence_check_${builtins.replaceStrings [ "-" ] [ "_" ] slug})"
  '';
  cadenceStartExec = slug: pkgs.writeShellScript "cadence-start-${slug}" ''
    exec ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 \
      "https://health.b.nel.family/ping/$(cat /run/agenix/cadence_check_${builtins.replaceStrings [ "-" ] [ "_" ] slug})/start"
  '';
  cadenceZfsScrubReport = slug: pkgs.writeShellScript "cadence-zfs-scrub-${slug}" ''
    set -u
    uuid="$(cat /run/agenix/cadence_check_${builtins.replaceStrings [ "-" ] [ "_" ] slug})"
    base="https://health.b.nel.family/ping/$uuid"
    result="''${SERVICE_RESULT:-unknown}"
    exit_code="''${EXIT_STATUS:-?}"
    status_output="$(${pkgs.zfs}/bin/zpool status 2>&1 || true)"
    errors_output="$(${pkgs.zfs}/bin/zpool status -x 2>&1 || true)"
    if [ "$result" = "success" ] && printf '%s\n' "$errors_output" | grep -q "all pools are healthy"; then
      url="$base"
    else
      url="$base/fail"
    fi
    body="SERVICE_RESULT=$result EXIT_STATUS=$exit_code

    zpool status -x:
    $errors_output

    zpool status:
    $status_output
    "
    ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 \
      --data-binary "$body" "$url" || true
  '';
in
{
  age.secrets.cadence_check_syncoid_vor_NelsonData.rekeyFile =
    ../../secrets/store/cadence/checks/syncoid-vor-NelsonData.age;
  age.secrets.cadence_check_zfs_scrub_vor.rekeyFile =
    ../../secrets/store/cadence/checks/zfs-scrub-vor.age;

  systemd.services.zfs-scrub.serviceConfig = {
    ExecStartPre = "+${cadenceStartExec "zfs-scrub-vor"}";
    ExecStopPost = "+${cadenceZfsScrubReport "zfs-scrub-vor"}";
  };

  services.sanoid = {
    enable = true;
    datasets = {
      "liveData/NelsonData" = {
        hourly = 72;
        daily = 31;
        weekly = 26;
        monthly = 12;
        yearly = 5;
        useTemplate = [ "common" ];
      };
      # Immich photos + its nightly DB dumps. romeo pulls these snapshots
      # (--no-sync-snap), so they are the only copy points it has.
      "liveData/level2" = {
        hourly = 72;
        daily = 31;
        weekly = 52;
        monthly = 24;
        yearly = 10;
        useTemplate = [ "common" ];
      };
      # Replication targets: syncoid never prunes the receiving side, so each
      # of these needs its own retention policy or snapshots accumulate forever.
      "vault/Backups/NelsonData" = {
        hourly = 72;
        daily = 31;
        weekly = 52;
        monthly = 24;
        yearly = 10;
        useTemplate = [ "common" ];
        autosnap = false;
      };
      "vault/Backups/bcnelson/level1" = {
        hourly = 72;
        daily = 31;
        weekly = 52;
        monthly = 24;
        yearly = 10;
        useTemplate = [ "common" ];
        autosnap = false;
      };
      "vault/Backups/bcnelson/level2" = {
        hourly = 72;
        daily = 31;
        weekly = 52;
        monthly = 24;
        yearly = 10;
        useTemplate = [ "common" ];
        autosnap = false;
      };
    };
    templates = {
      "common" = {
        autoprune = true;
        autosnap = true;
        recursive = true;
      };
    };
  };
  services.syncoid = {
    enable = true;
    commonArgs = [ "--debug" ];
    #https://github.com/NixOS/nixpkgs/pull/265543
    service.serviceConfig.PrivateUsers = lib.mkForce false;
    commands = {
      "liveData/NelsonData Local Backup" = {
        source = "liveData/NelsonData";
        target = "vault/Backups/NelsonData";
        service.serviceConfig.ExecStartPost = "+${cadencePingExec "syncoid-vor-NelsonData"}";
      };
    };
  };

  # Not at :00 with sanoid: syncoid's sync snapshot would land before the
  # hourly autosnap, so each run carried the previous hour's and
  # vault/Backups/NelsonData (and romeo's pull of it, at :10) ran 1-2h behind.
  systemd.timers."syncoid-liveData-NelsonData-Local-Backup".timerConfig.OnCalendar =
    lib.mkForce "*-*-* *:05:00";

  # romeo's syncoid runs these on vor over ssh (they must be on the login
  # PATH, not just the local syncoid units'): without mbuffer it warns and
  # sends/receives unbuffered.
  environment.systemPackages = [ pkgs.mbuffer pkgs.pv ];

  # User for syncoid to pull backups
  users.users.syncoid = {
    isSystemUser = true;
    description = "syncoid user";
    useDefaultShell = true;
  };

  systemd.timers.syncoid-zfs-allow = {
    enable = true;
    timerConfig = {
      OnBootSec = "1min";
      OnUnitActiveSec = "60m";
      Persistent = true;
    };
    wantedBy = [ "timers.target" ];
  };

  systemd.services.syncoid-zfs-allow = {
    enable = true;
    serviceConfig = {
      Type = "oneshot";
      User = "root";
      ExecStart = [
        "${pkgs.zfs}/bin/zfs allow -u syncoid send,receive,hold,mount,snapshot,destroy,create vault"
        # romeo only reads live data: send, never receive/destroy.
        "${pkgs.zfs}/bin/zfs allow -u syncoid send,hold liveData/level2"
      ];
    };
    restartIfChanged = false;
  };
}
