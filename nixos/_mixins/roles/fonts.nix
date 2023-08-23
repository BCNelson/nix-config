{ pkgs, ... }:
{
  fonts.fonts = [
    pkgs.nerdfonts
    pkgs.noto-fonts
    pkgs.noto-fonts-cjk
    pkgs.noto-fonts-emoji
  ];
}