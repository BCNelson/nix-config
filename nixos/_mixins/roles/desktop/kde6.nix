{ lib, pkgs, ... }:
{
  programs.partition-manager.enable = true;

  # A switch repoints /run/current-system/sw, which Plasma's app cache
  # (ksycoca) never notices: it decides staleness from directory mtimes, and
  # through the symlink every one of them is the store's 1. So a newly added
  # system package stays out of KRunner until it restarts. Bumping a scanned
  # directory outside the store makes every Plasma process rebuild the cache
  # itself on its next lookup. The plasma6 module's own fix, deleting the
  # caches, only reaches a KRunner once per run of it; see the longer note on
  # home-manager/bcnelson/_mixins/kde.nix's touchApplicationsDir.
  #
  # userActivationScripts run as each logged-in user, after the system profile
  # has been switched.
  system.userActivationScripts.touchApplicationsDir = ''
    mkdir -p "''${XDG_DATA_HOME:-$HOME/.local/share}/applications"
    touch "''${XDG_DATA_HOME:-$HOME/.local/share}/applications"
  '';
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
