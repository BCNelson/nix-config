{ lib, pkgs, ... }:
{
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
      "vault/Backups/Nelson Family Data" = {
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
      };
    };
  };

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
      ExecStart = "${pkgs.zfs}/bin/zfs allow -u syncoid send,receive,hold,mount,snapshot,destroy,create vault";
    };
    restartIfChanged = false;
  };
}
