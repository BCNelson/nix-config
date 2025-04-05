{ pkgs, ... }:

{
  imports = [
    ../_mixins/programs/deckmaster
    ../_mixins/programs/libreOffice.nix
    ../_mixins/programs/emulator.nix
    ../_mixins/programs/blender.nix
    ./_mixins/workstation.nix
  ];

  home.packages = [
    pkgs.android-tools
    pkgs.nixpkgs-review
    pkgs.ventoy-full
    pkgs.libation
    pkgs.androidStudioPackages.canary
  ];
}
