{ pkgs, ... }:
let
  shared = import ./shared.nix;
  inherit (shared) plugins;
in
{
  home.packages = with pkgs.unstable; [
    (jetbrains.plugins.addPlugins jetbrains.goland plugins)
    go
    gcc
    binutils
  ];
}
