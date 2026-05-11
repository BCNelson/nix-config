{ config, pkgs, ... }:
let
  ntfyTopicFile = "/run/agenix/happy_ntfy_topic";
  happyHomeDir = "${config.xdg.dataHome}/happy";

  happy-coder = pkgs.symlinkJoin {
    name = "happy-coder-wrapped-${pkgs.happy-coder.version}";
    paths = [ pkgs.happy-coder ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      wrapProgram $out/bin/happy \
        --set-default HAPPY_HOME_DIR ${happyHomeDir}
      wrapProgram $out/bin/happy-mcp \
        --set-default HAPPY_HOME_DIR ${happyHomeDir}
    '';
    inherit (pkgs.happy-coder) meta;
  };
in
{
  home.packages = [
    happy-coder
    pkgs.happy-auth-notify
  ];

  systemd.user.services.happy-auth-bootstrap = {
    Unit = {
      Description = "Bootstrap happy authentication via ntfy notification";
      After = [ "network-online.target" ];
      Wants = [ "network-online.target" ];
      Before = [ "happy-daemon.service" ];
      ConditionPathExists = "!${happyHomeDir}/access.key";
    };

    Service = {
      Type = "oneshot";
      RemainAfterExit = true;
      Environment = [
        "HAPPY_HOME_DIR=${happyHomeDir}"
        "NTFY_TOPIC_FILE=${ntfyTopicFile}"
      ];
      ExecStart = "${pkgs.happy-auth-notify}/bin/happy-auth-notify";
      TimeoutStartSec = "1h";
    };

    Install = {
      WantedBy = [ "default.target" ];
    };
  };

  systemd.user.services.happy-daemon = {
    Unit = {
      Description = "Happy remote agent daemon";
      After = [ "network-online.target" "happy-auth-bootstrap.service" ];
      Wants = [ "network-online.target" "happy-auth-bootstrap.service" ];
    };

    Service = {
      Environment = [ "PATH=${config.home.profileDirectory}/bin" ];
      ExecStart = "${happy-coder}/bin/happy daemon start-sync";
      Restart = "on-failure";
      RestartSec = 5;
    };

    Install = {
      WantedBy = [ "default.target" ];
    };
  };
}
