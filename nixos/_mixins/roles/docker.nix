{ pkgs, ... }:
{
  virtualisation.docker.enable = true;
  virtualisation.docker.daemon.settings = {
    shutdown-timeout = 15;
  };

  systemd.services.docker.serviceConfig = {
    KillMode = "mixed";
    TimeoutStopSec = 30;
  };

  environment.systemPackages = [
    pkgs.docker-compose
  ];
}
