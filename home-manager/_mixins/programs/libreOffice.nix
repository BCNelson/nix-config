{ config, pkgs, ... }:
{
  home.packages = [
    (config.lib.nixGL.wrap pkgs.libreoffice-qt6-fresh)
    pkgs.hunspell
    pkgs.hunspellDicts.en_US
  ];
}
