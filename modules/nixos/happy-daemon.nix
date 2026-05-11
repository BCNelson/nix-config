{ config, lib, pkgs, ... }:
let
  cfg = config.services.bcnelson.happy-daemon;
in
{
  options.services.bcnelson.happy-daemon = {
    enable = lib.mkEnableOption "Happy remote agent daemon";

    user = lib.mkOption {
      type = lib.types.str;
      description = "User to run the daemon as. The daemon spawns claude/codex on behalf of this user.";
    };

    package = lib.mkPackageOption pkgs "happy-coder" { };

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
  };

  config = lib.mkIf cfg.enable {
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
  };
}
