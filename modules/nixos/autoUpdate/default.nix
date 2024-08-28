{ config, lib, pkgs, ... }:

let

  cfg = config.services.bcnelson.autoUpdate;
  autoUpdateScript = pkgs.writeShellApplication {
    name = "auto-update";
    runtimeInputs = with pkgs; [ git gnupg git-crypt coreutils just bash nix nixos-rebuild systemd curl hostname libnotify ];
    text = builtins.readFile ./auto-update.sh;
  };

in
{
  options = {
    services.bcnelson.autoUpdate = {
      enable = lib.mkEnableOption "Enable autostarting of applications";
      path = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Path to the git repository";
      };
      healthCheckUuid = lib.mkOption {
        type = lib.types.str;
        default = "";
        description = "Health check configuration";
      };
      healthCheck = {
        enable = lib.mkEnableOption "Enable health check";
        url = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "Health check URL";
        };
        interval = lib.mkOption {
          type = lib.types.str;
          default = "5m";
          description = "Health check interval";
        };
      };
      ntfy = {
        enable = lib.mkEnableOption "Enable ntfy";
        topic = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "Ntfy topic";
        };
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

    assertions = [
      {
        assertion = !cfg.healthCheck.enable || cfg.healthCheck.url != "";
        message = "Health check URL must be set if health check is enabled";
      }
      {
        assertion = !cfg.ntfy.enable || cfg.ntfy.topic != "";
        message = "Ntfy topic must be set if ntfy is enabled";
      }
      {
        assertion = cfg.path != null && cfg.path != "";
        message = "Path must be set";
      }
    ];

    systemd.services.auto-update = {
      enable = true;
      environment = {
        NTFY_TOPIC = if cfg.ntfy.enable then cfg.ntfy.topic else "";
        HEALTHCHECK_UUID = if cfg.healthCheck.enable then cfg.healthCheckUuid else "";
        HEALTHCHECK_URL = if cfg.healthCheck.enable then cfg.healthCheck.url else "";
        REBOOT = if cfg.reboot then "true" else "false";
        CONFIG_PATH = cfg.path;
      };
      serviceConfig = {
        Type = "oneshot";
        User = "root";
        ExecStart = "${autoUpdateScript}/bin/auto-update";
        TimeoutStartSec = "6h";
      };
      restartIfChanged = false;
    };

    systemd.services.startup-notify = lib.mkIf cfg.ntfy.enable {
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
