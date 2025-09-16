{ lib, pkgs, ... }:
{
  programs.partition-manager.enable = true;
} // (
  if lib.strings.versionAtLeast lib.trivial.release "24.05" then {
    xdg.portal.extraPortals = [ pkgs.kdePackages.xdg-desktop-portal-kde ];
    services.desktopManager.plasma6.enable = true;
    services.displayManager.defaultSession = "plasma";
    environment.systemPackages = [
      pkgs.kdePackages.polkit-kde-agent-1
    ];
    services.displayManager.sddm = {
      enable = true;
      autoNumlock = true;
      wayland = {
        enable = true;
        compositor = "kwin";
      };
    };
  } else {
    services.desktopManager.plasma6.enable = true;
    services.xserver.displayManager.defaultSession = "plasma";
    services.xserver.displayManager.lightdm.enable = lib.mkForce false;
    services.displayManager.sddm = {
      enable = lib.mkForce true;
      autoNumlock = true;
      wayland = {
        enable = true;
        compositor = "kwin";
      };
    };

  }
)
