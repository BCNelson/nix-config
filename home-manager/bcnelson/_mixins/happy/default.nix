{ config, pkgs, ... }:
{
  home.packages = [
    pkgs.happy-coder
  ];

  systemd.user.services.happy-daemon = {
    Unit = {
      Description = "Happy remote agent daemon";
      After = [ "network-online.target" ];
      Wants = [ "network-online.target" ];
    };

    Service = {
      Environment = [ "PATH=${config.home.profileDirectory}/bin" ];
      ExecStart = "${pkgs.happy-coder}/bin/happy daemon start-sync";
      Restart = "on-failure";
      RestartSec = 5;
    };

    Install = {
      WantedBy = [ "default.target" ];
    };
  };
}
