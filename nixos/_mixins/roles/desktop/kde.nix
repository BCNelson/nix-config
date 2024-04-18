{ pkgs, lib, ... }:
{
  services = {} // (if lib.strings.versionAtLeast lib.trivial.release "24.05" then {
    xserver.desktopManager.plasma5.enable = true;
    displayManager.defaultSession = "plasmawayland";
  } else {
    xserver.desktopManager.plasma5.enable = true;
    xserver.displayManager.defaultSession = "plasmawayland";
  });

  environment.plasma5.excludePackages = with pkgs.libsForQt5; [
    elisa
    oxygen
  ];

  programs.partition-manager.enable = true;
}
