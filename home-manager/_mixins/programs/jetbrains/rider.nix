{ pkgs, ... }:
let
  shared = import ./shared.nix;
  inherit (shared) plugins;
in
{
  home.packages = with pkgs.unstable; [
    (jetbrains.plugins.addPlugins jetbrains.rider plugins)
    dotnet-sdk_7
  ];
}
