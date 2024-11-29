{ config, lib, pkgs, ... }:

let

  cfg = config.services.bcnelson.autoUpdate;
  autoUpdateScript = pkgs.writeShellApplication {
    name = "auto-update";
    runtimeInputs = with pkgs; [
      git
      gnupg
      git-crypt
      coreutils
      just
      bash
      nix
      nixos-rebuild
      systemd
      curl
      hostname
      libnotify
      openssh
      sudo
    ];
    text = builtins.readFile ./auto-update.sh;
  };

  ntfy-refresh-client = pkgs.writeShellApplication {
    name = "ntfy-refresh-client";
    runtimeInputs = with pkgs; [ bash ntfy-sh systemd ];
    text = ''
      #!/usr/bin/env bash
      echo "Starting ntfy-refresh-client"
      NTFY_REFRESH_TOPIC = "$(cat $NTFY_REFRESH_TOPIC_FILE)"
      # shellcheck disable=SC2016
      ntfy subscribe "$NTFY_REFRESH_TOPIC" 'echo "Starting Update"; systemctl start auto-update.service'
    '';
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
      healthCheck = {
        enable = lib.mkEnableOption "Enable health check";
        url = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "Health check URL";
        };
        uuidFile = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "Health check configuration";
        };
      };
      ntfy = {
        enable = lib.mkEnableOption "Enable ntfy";
        topicFile = lib.mkOption {
          type = lib.types.str;
          default = "";
          description = "Ntfy topic";
        };
      };
      ntfy-refresh = {
        enable = lib.mkEnableOption "Enable ntfy pushed based refresh";
        topicFile = lib.mkOption {
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
      user = lib.mkOption {
        type = lib.types.str;
        default = "root";
        description = "User to run the service as";
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
        assertion = !cfg.healthCheck.enable || cfg.healthCheck.uuid != "";
        message = "Health check UUID must be set if health check is enabled";
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
        NTFY_TOPIC_FILE = if cfg.ntfy.enable then cfg.ntfy.topicFile else "";
        HEALTHCHECK_UUID_FILE = if cfg.healthCheck.enable then cfg.healthCheck.uuidFile else "";
        HEALTHCHECK_URL = if cfg.healthCheck.enable then cfg.healthCheck.url else "";
        REBOOT = if cfg.reboot then "true" else "false";
        CONFIG_PATH = cfg.path;
        GIT_COMMITTER_EMAIL = "admin@nel.family";
        GIT_COMMITTER_NAME = "Admin";
        USER = cfg.user;
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
            https://ntfy.sh/${cfg.ntfy.topic}
      '';
      restartIfChanged = false;
    };

    systemd.services.autoupdate-ntfy-client = lib.mkIf cfg.ntfy-refresh.enable {
      enable = true;
      wantedBy = [ "multi-user.target" ];
      wants = [ "network-online.target" ];
      environment = {
        NTFY_REFRESH_TOPIC_FILE = cfg.ntfy-refresh.topicFile;
      };
      serviceConfig = {
        Type = "simple";
        User = "root";
        ExecStart = "${ntfy-refresh-client}/bin/ntfy-refresh-client";
      };
      restartIfChanged = true;
    };
  };
}
