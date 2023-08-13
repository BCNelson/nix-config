{ pkgs, lib, ... }:
{
  services.xserver.desktopManager.plasma5.enable = true;

  services.xserver.displayManager.defaultSession = lib.mkDefault "plasmawayland";

  environment.plasma5.excludePackages = with pkgs.libsForQt5; [
    elisa
    oxygen
  ];
}
