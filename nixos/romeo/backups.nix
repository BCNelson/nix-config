{ lib, pkgs, libx, ... }:
let
  basicBorgJob = { repo, paths }: {
    inherit repo paths;
    encryption.mode = "none";
    environment.BORG_RSH = "ssh -o 'StrictHostKeyChecking=no' -i /root/.ssh/id_ed25519";
    environment.BORG_UNKNOWN_UNENCRYPTED_REPO_ACCESS_IS_OK = "yes";
    extraCreateArgs = "--verbose --stats --checkpoint-interval 600";
    compression = "zstd,1";
    startAt = "daily";
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
      "scary/replaceable" = {
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
      };
    };
  };

  services.borgbackup.jobs = {
    level1 = basicBorgJob {
      repo = borgReposSecrets.level1;
      paths = "/mnt/vault/data/level1";
    };
    level2 = basicBorgJob {
      repo = borgReposSecrets.level2;
      paths = "/mnt/vault/data/level2";
    };
    level3 = basicBorgJob {
      repo = borgReposSecrets.level3;
      paths = "/mnt/vault/data/level3";
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
    script = with pkgs; ''
      sudo zfs allow -u backup send,hold
    '';
  };
}
