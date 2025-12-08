{ pkgs, lib, ... }:

{
  # Restore KDE apps excluded system-wide on bravo
  home.packages = with pkgs.kdePackages; [
    konsole
    ark
    dolphin
    dolphin-plugins
    baloo-widgets
    gwenview
    okular
    spectacle
    elisa
    khelpcenter
    ffmpegthumbs
    krdp
    plasma-browser-integration
    (lib.getBin qttools)
  ];
}
