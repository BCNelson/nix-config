{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/audacity.nix
    ./_mixins/workstation.nix
    ./_mixins/mcp/home-assistant.nix
  ];

  home.packages = [
    pkgs.yt-dlp
    pkgs.libation
    pkgs.mb4-extractor
  ];
}
