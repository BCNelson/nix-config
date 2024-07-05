{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/audacity.nix
  ];

  home.packages = [
    pkgs.yt-dlp
  ];

}
