{pkgs, ...}:
let
  shared = import ./shared.nix;
  plugins = [] ++ shared.plugins;
in {
  home.packages = with pkgs.unstable; [
    (jetbrains.plugins.addPlugins jetbrains.goland plugins)
    go
    gcc
    binutils
  ];
}
