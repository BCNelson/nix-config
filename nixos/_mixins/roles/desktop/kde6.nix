{ lib, pkgs, ... }:
{
  services.desktopManager.plasma6.enable = true;

  programs.partition-manager.enable = true;

  xdg.portal.extraPortals = [ pkgs.xdg-desktop-portal-kde ];
} // (
  if lib.strings.versionAtLeast lib.trivial.release "24.05" then {
    services.desktopManager.plasma6.enable = true;
    services.displayManager.defaultSession = "plasma";
  } else {
    services.xserver.displayManager.defaultSession = "plasma";
  }
)
