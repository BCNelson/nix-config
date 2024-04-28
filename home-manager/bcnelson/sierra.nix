{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/deckmaster
    ../_mixins/programs/jetbrains/dataGrip.nix
    ../_mixins/programs/jetbrains/goland.nix
    ../_mixins/programs/emulator.nix
  ];

  home.packages = [
    pkgs.winbox
    pkgs.android-tools
    pkgs.nixpkgs-review
  ];
}
