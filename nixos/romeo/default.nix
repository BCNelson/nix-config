args@{ pkgs, libx, ... }:
let
  dataDirs = import ./dataDirs.nix;
  services = import ./services { inherit libx dataDirs pkgs; };
  healthcheckUuid = libx.getSecret ./sensitive.nix "auto_update_healthCheck_uuid";
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      (import ../_mixins/autoupdate (args // { inherit healthcheckUuid; }))
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ../_mixins/roles/figurine.nix
      ./unbound.nix
      ./backups.nix
      ./nfs.nix
      ./monitoring.nix
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
}
