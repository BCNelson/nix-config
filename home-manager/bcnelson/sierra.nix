{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/deckmaster
    ../_mixins/programs/libreOffice.nix
    ../_mixins/programs/emulator.nix
    ../_mixins/programs/handbrake.nix
  ];

  home.packages = [
    pkgs.winbox
    pkgs.android-tools
    pkgs.nixpkgs-review
    pkgs.ventoy-full
    pkgs.libation
  ];
}
