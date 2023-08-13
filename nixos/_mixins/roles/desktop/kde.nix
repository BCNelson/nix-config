{ pkgs, ... }:
{
  services.xserver.desktopManager.plasma5.enable = true;

  services.xserver.displayManager.defaultSession = "plasmawayland";

  environment.plasma5.excludePackages = with pkgs.libsForQt5; [
    elisa
    oxygen
  ];
}
