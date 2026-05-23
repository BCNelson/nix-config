{ config, lib, pkgs, ... }:
let
  cfg = config.services.bcnelson.happy-daemon;
in
{
  options.services.bcnelson.happy-daemon = {
    enable = lib.mkEnableOption "Happy remote agent daemon";

    user = lib.mkOption {
      type = lib.types.str;
      description = "User to run the daemon and auth bootstrap as.";
    };

    package = lib.mkPackageOption pkgs "happy-coder" { };

    authNotifyPackage = lib.mkPackageOption pkgs "happy-auth-notify" { };

    claudePackage = lib.mkPackageOption pkgs "claude-code-bin" { };

    happyHomeDir = lib.mkOption {
      type = lib.types.str;
      default = "/home/${cfg.user}/.local/share/happy";
      defaultText = lib.literalExpression ''"/home/''${user}/.local/share/happy"'';
      description = "Directory containing happy state (access.key, settings.json, daemon state, logs).";
    };

    extraPackages = lib.mkOption {
      type = lib.types.listOf lib.types.package;
      default = [ ];
      example = lib.literalExpression "with pkgs; [ claude-code-bin codex ]";
      description = "Packages whose bin/ dirs are prepended to the daemon's PATH so it can find claude/codex.";
    };

    ntfyTopicFile = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = lib.literalExpression "config.age.secrets.happy_ntfy_topic.path";
      description = ''
        Path to a file (readable by the daemon user) containing an ntfy topic. If set, a
        happy-auth-bootstrap oneshot does the phone pairing dance and pushes the
        pairing URL to https://ntfy.sh/&lt;topic&gt; whenever access.key is missing.
      '';
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      systemd.services.happy-daemon = {
        description = "Happy remote agent daemon (${cfg.user})";
        after = [ "network-online.target" ];
        wants = [ "network-online.target" ];
        wantedBy = [ "multi-user.target" ];

        unitConfig.ConditionPathExists = "${cfg.happyHomeDir}/access.key";

        path = cfg.extraPackages;

        serviceConfig = {
          Type = "simple";
          User = cfg.user;
          WorkingDirectory = "/home/${cfg.user}";
          Environment = [
            "HOME=/home/${cfg.user}"
            "HAPPY_HOME_DIR=${cfg.happyHomeDir}"
            "HAPPY_CLAUDE_PATH=${cfg.claudePackage}/bin/claude"
          ];
          ExecStart = "${cfg.package}/bin/happy daemon start-sync";
          Restart = "on-failure";
          RestartSec = 10;
        };
      };

      systemd.paths.happy-daemon = {
        description = "Start happy-daemon when access.key appears (${cfg.user})";
        wantedBy = [ "multi-user.target" ];
        pathConfig = {
          PathExists = "${cfg.happyHomeDir}/access.key";
          Unit = "happy-daemon.service";
        };
      };
    }

    (lib.mkIf (cfg.ntfyTopicFile != null) {
      systemd.services.happy-auth-bootstrap = {
        description = "Bootstrap happy authentication via ntfy notification (${cfg.user})";
        after = [ "network-online.target" ];
        wants = [ "network-online.target" ];
        wantedBy = [ "multi-user.target" ];

        unitConfig.ConditionPathExists = "!${cfg.happyHomeDir}/access.key";

        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          User = cfg.user;
          WorkingDirectory = "/home/${cfg.user}";
          Environment = [
            "HOME=/home/${cfg.user}"
            "HAPPY_HOME_DIR=${cfg.happyHomeDir}"
            "NTFY_TOPIC_FILE=${cfg.ntfyTopicFile}"
          ];
          ExecStart = "${cfg.authNotifyPackage}/bin/happy-auth-notify";
          TimeoutStartSec = "1h";
        };
      };
    })
  ]);
}
