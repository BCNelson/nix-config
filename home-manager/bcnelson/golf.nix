{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/audacity.nix
    ../_mixins/work/redo.nix
  ];

  home.packages = [
    pkgs.yt-dlp
  ];
}
