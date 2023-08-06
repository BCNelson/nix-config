{ pkgs, ... }:
{
  services.xserver.desktopManager.plasma5.enable = true;

  environment.plasma5.excludePackages = with pkgs.libsForQt5; [
    elisa
    oxygen
  ];
}
