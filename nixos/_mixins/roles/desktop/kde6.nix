_:
{
  services.desktopManager.plasma6.enable = true;

  services.xserver.displayManager.defaultSession = "plasma";

  programs.partition-manager.enable = true;
}
