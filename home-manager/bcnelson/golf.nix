{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/audacity.nix
    ./_mixins/workstation.nix
  ];

  home.packages = [
    pkgs.yt-dlp
    pkgs.libation
    pkgs.mb4-extractor
  ];
}
