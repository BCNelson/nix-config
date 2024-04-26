args@{ pkgs, libx, ... }:
let
  dataDirs = {
    level1 = "/data/level1"; # Critical
    level2 = "/data/level2"; # Important
    level3 = "/data/level3"; # High
    level4 = "/data/level4"; # Medium
    level5 = "/data/level5"; # Low
    level6 = "/data/level6"; # Replaceable
    level7 = "/data/level6"; # Ephemeral
  };
  services = import ./services { inherit libx dataDirs pkgs; };
  healthcheckUuid = libx.getSecret ./sensitive.nix "auto_update_healthCheck_uuid";
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      (import ../_mixins/autoupdate (args // { inherit healthcheckUuid;}))
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
    ];
  environment.systemPackages = [
    services.networkBacked
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
  networking.hostId = "9a637b7f";
}
