{ ... }:
{
  imports = [];

  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/config";
    reboot = false;
    refreshInterval = "5m";
  };

}
