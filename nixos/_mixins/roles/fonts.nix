{ pkgs, ... }:
{
  fonts.packages = [
    pkgs.nerdfonts
    pkgs.noto-fonts
    pkgs.noto-fonts-cjk-sans
    pkgs.noto-fonts-emoji
  ];
}
