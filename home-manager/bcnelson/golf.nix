{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/audacity.nix
  ];

  home.packages = [
    pkgs.yt-dlp
    pkgs.libation
    pkgs.mb4-extractor
  ];
}
