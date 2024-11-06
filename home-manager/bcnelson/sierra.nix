{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/deckmaster
    ../_mixins/programs/libreOffice.nix
    ../_mixins/programs/emulator.nix
  ];

  home.packages = [
    pkgs.winbox
    pkgs.android-tools
    pkgs.nixpkgs-review
    pkgs.ventoy-full
    pkgs.libation
  ];
}
