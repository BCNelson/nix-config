{ config, lib, pkgs, libx, ... }:
let
  cadencePreHook = slug: ''
    ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 \
      "https://health.b.nel.family/ping/$(cat /run/agenix/cadence_check_${builtins.replaceStrings [ "-" ] [ "_" ] slug})/start" \
      || true
  '';
  cadencePostHook = slug: ''
    url="https://health.b.nel.family/ping/$(cat /run/agenix/cadence_check_${builtins.replaceStrings [ "-" ] [ "_" ] slug})"
    if [ "$exitStatus" -ne 0 ]; then url="$url/fail"; fi
    ${pkgs.systemd}/bin/journalctl _SYSTEMD_INVOCATION_ID="$INVOCATION_ID" \
        --no-pager --no-hostname -o short-iso 2>/dev/null \
      | tail -n 200 | tail -c 9000 \
      | ${pkgs.curl}/bin/curl -fsS -m 10 --retry 2 --retry-delay 2 \
          -H "Content-Type: text/plain; charset=utf-8" \
          --data-binary @- \
          "$url" || true
  '';
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
  basicBorgJob = { repo, paths, cadenceSlug, prune ? null }: {
    inherit repo paths;
    encryption.mode = "none";
    environment.BORG_RSH = "ssh -o 'StrictHostKeyChecking=no' -i ${config.age.secrets.borgbaseSshKey.path}";
    environment.BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK = "yes";
    extraCreateArgs = "--verbose --stats --checkpoint-interval 600";
    compression = "zstd,1";
    startAt = "*-*-* 0/6:00:00";
    prune = lib.mkIf (prune != null) prune;
    preHook = cadencePreHook cadenceSlug;
    postHook = cadencePostHook cadenceSlug;
  };
  borgReposSecrets = libx.getSecretWithDefault ./sensitive.nix "borgRepos" {
    level1 = "";
    level2 = "";
    level3 = "";
    level4 = "";
    level5 = "";
  };
in
{
  services.sanoid = {
    enable = true;
    datasets = {
      "vault/data/level1" = {
        hourly = 72;
        daily = 31;
        weekly = 52;
        monthly = 24;
        yearly = 10;
        useTemplate = [ "common" ];
      };
      "vault/data/level2" = {
        hourly = 72;
        daily = 31;
        weekly = 52;
        monthly = 24;
        yearly = 10;
        useTemplate = [ "common" ];
      };
      "vault/data/level3" = {
        hourly = 72;
        daily = 31;
        weekly = 24;
        monthly = 12;
        yearly = 2;
        useTemplate = [ "common" ];
      };
      "vault/data/level4" = {
        hourly = 72;
        daily = 31;
        weekly = 24;
        monthly = 12;
        yearly = 2;
        useTemplate = [ "common" ];
      };
      "vault/data/level5" = {
        hourly = 72;
        daily = 31;
        weekly = 24;
        monthly = 12;
        yearly = 2;
        useTemplate = [ "common" ];
      };
      "trove/replaceable" = {
        hourly = 24;
        daily = 31;
        weekly = 8;
        monthly = 6;
        yearly = 1;
        useTemplate = [ "common" ];
      };
      "vault/remotebackups/VorNelsonData" = {
        hourly = 72;
        daily = 31;
        weekly = 26;
        monthly = 12;
        yearly = 5;
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
      "vor/vault/Backups/NelsonData" = {
        source = "syncoid@vor.ck.nel.family:vault/Backups/NelsonData";
        target = "vault/remotebackups/VorNelsonData";
        extraArgs = [
          "--compress=zstd-slow"
          "--source-bwlimit=15m"
          "--debug"
          "--sshoption=StrictHostKeyChecking=off"
        ];
        service.serviceConfig.ExecStartPost = "+${cadencePingExec "syncoid-romeo-NelsonData"}";
      };
      "vor/vault/Backups/level1" = {
        source = "vault/data/level1";
        target = "syncoid@vor.ck.nel.family:vault/Backups/bcnelson/level1";
        extraArgs = [
          "--compress=zstd-slow"
          "--source-bwlimit=15m"
          "--debug"
          "--sshoption=StrictHostKeyChecking=off"
        ];
        service.serviceConfig.ExecStartPost = "+${cadencePingExec "syncoid-romeo-level1"}";
      };
      "vor/vault/Backups/level2" = {
        source = "vault/data/level2";
        target = "syncoid@vor.ck.nel.family:vault/Backups/bcnelson/level2";
        extraArgs = [
          "--compress=zstd-slow"
          "--source-bwlimit=15m"
          "--debug"
          "--sshoption=StrictHostKeyChecking=off"
        ];
        service.serviceConfig.ExecStartPost = "+${cadencePingExec "syncoid-romeo-level2"}";
      };
    };
  };

  age.secrets.borgbaseSshKey.rekeyFile = ../../secrets/store/romeo/borgbase_ssh_key.age;

  age.secrets.cadence_check_borgbackup_romeo_level1.rekeyFile =
    ../../secrets/store/cadence/checks/borgbackup-romeo-level1.age;
  age.secrets.cadence_check_borgbackup_romeo_level2.rekeyFile =
    ../../secrets/store/cadence/checks/borgbackup-romeo-level2.age;
  age.secrets.cadence_check_borgbackup_romeo_level3.rekeyFile =
    ../../secrets/store/cadence/checks/borgbackup-romeo-level3.age;
  age.secrets.cadence_check_syncoid_romeo_NelsonData.rekeyFile =
    ../../secrets/store/cadence/checks/syncoid-romeo-NelsonData.age;
  age.secrets.cadence_check_syncoid_romeo_level1.rekeyFile =
    ../../secrets/store/cadence/checks/syncoid-romeo-level1.age;
  age.secrets.cadence_check_syncoid_romeo_level2.rekeyFile =
    ../../secrets/store/cadence/checks/syncoid-romeo-level2.age;
  age.secrets.cadence_check_zfs_scrub_romeo.rekeyFile =
    ../../secrets/store/cadence/checks/zfs-scrub-romeo.age;

  systemd.services.zfs-scrub.serviceConfig = {
    ExecStartPre = "+${cadenceStartExec "zfs-scrub-romeo"}";
    ExecStopPost = "+${cadenceZfsScrubReport "zfs-scrub-romeo"}";
  };

  services.borgbackup.jobs = {
    level1 = basicBorgJob {
      repo = borgReposSecrets.level1;
      paths = "/mnt/vault/data/level1";
      cadenceSlug = "borgbackup-romeo-level1";
      prune.keep = {
        within = "7d";
        daily = 31;
        weekly = -1;
      };
    };
    level2 = basicBorgJob {
      repo = borgReposSecrets.level2;
      paths = "/mnt/vault/data/level2";
      cadenceSlug = "borgbackup-romeo-level2";
      prune.keep = {
        within = "7d";
        daily = 7;
        weekly = 4;
        monthly = 12;
        yearly = 5;
      };
    };
    level3 = basicBorgJob {
      repo = borgReposSecrets.level3;
      paths = "/mnt/vault/data/level3";
      cadenceSlug = "borgbackup-romeo-level3";
      prune.keep = {
        within = "7d";
        daily = 7;
        weekly = 4;
        monthly = 6;
      };
    };
    # level4 = basicBorgJob {
    #   repo = borgReposSecrets.level4;
    #   paths = "/mnt/vault/data/level4";
    # };
    # level5 = basicBorgJob {
    #   repo = borgReposSecrets.level5;
    #   paths = "/mnt/vault/data/level5";
    # };
  };

  users.users.backup = {
    isNormalUser = true;
    description = "Backup user";
  };

  systemd.services.BackupZFSAlow = {
    serviceConfig.Type = "oneshot";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    script = ''
      sudo zfs allow -u backup send,hold
    '';
  };
}
