{ config, pkgs, ... }:
{
  home.packages = [
    (config.lib.nixGL.wrap pkgs.libreoffice-qt)
    pkgs.hunspell
    pkgs.hunspellDicts.en_US
  ];
}
