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

  # Bound shutdown so a hung container (e.g. orphaned s6-svscan from a
  # linuxserver.io image after its volumes have unmounted) can't pin the box
  # in systemd-shutdown forever — RebootWatchdogSec arms the hardware
  # watchdog during the final reboot phase.
  systemd.settings.Manager = {
    DefaultTimeoutStopSec = "45s";
    RebootWatchdogSec = "60s";
  };

  environment.systemPackages = [
    pkgs.docker-compose
  ];
}
