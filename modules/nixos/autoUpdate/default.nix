{ config, lib, pkgs, ... }:

let

  cfg = config.services.bcnelson.autoUpdate;
  autoUpdateScript = pkgs.writeShellApplication {
    name = "auto-update";
    runtimeInputs = with pkgs; [ git gnupg git-crypt coreutils just bash nix nixos-rebuild systemd curl hostname libnotify];
    text = builtins.readFile ./auto-update.sh;
  };

in
{
  options = {
    services.bcnelson.autoUpdate = {
      enable = lib.mkEnableOption "Enable autostarting of applications";
      healthCheckUuid = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = "Health check configuration";
      };
      ntfyTopic = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = "Ntfy topic";
      };
        reboot = lib.mkOption {
            type = lib.types.bool;
            default = false;
            description = "Reboot after update";
        };
        refreshInterval = lib.mkOption {
            type = lib.types.str;
            default = "15m";
            description = "Refresh interval";
        };
    };
  };

  config = lib.mkIf cfg.enable {
    systemd.timers.auto-update = {
      enable = true;
      timerConfig = {
        OnBootSec = "1min";
        OnUnitActiveSec = cfg.refreshInterval;
        Persistent = true;
      };
      wantedBy = [ "timers.target" ];
    };

    systemd.services.auto-update = {
      enable = true;
      environment = {
        NTFY_TOPIC = cfg.ntfyTopic;
        HEALTHCHECK_UUID = cfg.healthCheckUuid;
        REBOOT = if cfg.reboot then "true" else "false";
      };
      serviceConfig = {
        Type = "oneshot";
        User = "root";
        ExecStart = "${autoUpdateScript}/bin/auto-update";
        TimeoutStartSec = "6h";
      };
      restartIfChanged = false;
    };

    systemd.services.startup-notify = {
      enable = true;
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      serviceConfig = {
        Type = "simple";
        RemainAfterExit = true;
      };
      script = ''
        #!/usr/bin/env bash
        ${pkgs.curl}/bin/curl -H "X-Title: $HOSTNAME has Started" \
            -H "X-Priority: 2" \
            -d "$HOSTNAME Has Booted!" \
            https://ntfy.sh/${cfg.ntfyTopic}
      '';
      restartIfChanged = false;
    };
  };
}
