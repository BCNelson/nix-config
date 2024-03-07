{ pkgs, libx, ... }:
let
  dataDirs = {
    level1 = "/mnt/vault/data/level1"; # Critical
    level2 = "/mnt/vault/data/level2"; # Important
    level3 = "/mnt/vault/data/level3"; # High
    level4 = "/mnt/vault/data/level4"; # Medium
    level5 = "/mnt/vault/data/level5"; # Low
    level6 = "/data/replaceable"; # Replaceable
    level7 = "/cache"; # Ephemeral
  };
  services = import ./services { inherit libx dataDirs pkgs; };
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/autoupdate
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ../_mixins/roles/figurine.nix
      ./unbound.nix
      ./backups.nix
    ];

  environment.systemPackages = [
    pkgs.zfs
    services.networkBacked
    pkgs.gparted
  ];

  systemd.timers.auto-update-services = {
    enable = true;
    timerConfig = {
      OnBootSec = "30min";
      OnUnitActiveSec = "60m";
      Persistent = true;
    };
    wantedBy = [ "timers.target" ];
  };

  systemd.services.auto-update-services = {
    enable = true;
    path = [ pkgs.docker ];
    serviceConfig = {
      Type = "oneshot";
      User = "root";
      ExecStart = "${services.networkBacked}/bin/dockerStack-general up -d --remove-orphans --pull always --quiet-pull";
    };
    restartTriggers = [ services.networkBacked ];
    restartIfChanged = false;
  };

  systemd.network = {
    netdevs = {
      "20-vlan10" = {
        netdevConfig = {
          Kind = "vlan";
          Name = "vlan10";
        };
        vlanConfig.Id = 10;
      };
      "20-vlan30" = {
        netdevConfig = {
          Kind = "vlan";
          Name = "vlan30";
        };
        vlanConfig.Id = 30;
      };
    };
    networks = {
      "30-enp11s0" = {
        matchConfig.Name = "enp11s0";
        vlan = [
          "vlan10"
          "vlan30"
        ];
        networkConfig.LinkLocalAddressing = "no";
        linkConfig.RequiredForOnline = "carrier";
      };
      "40-vlan10" = {
        matchConfig.Name = "vlan10";
      };
      "40-vlan30" = {
        matchConfig.Name = "vlan30";
      };
    };
  };
}
