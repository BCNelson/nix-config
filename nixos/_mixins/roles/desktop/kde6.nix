{ lib, pkgs, ... }:
{
  programs.partition-manager.enable = true;
} // (
  if lib.strings.versionAtLeast lib.trivial.release "24.05" then {
    xdg.portal.extraPortals = [ pkgs.xdg-desktop-portal-kde ];
    services.desktopManager.plasma6.enable = true;
    services.displayManager.defaultSession = "plasma";
  } else {
    services.desktopManager.plasma6.enable = true;
    services.xserver.displayManager.defaultSession = "plasma";
  }
)
