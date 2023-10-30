{pkgs, ...}:

{
  home.packages = with pkgs.unstable; [
    (jetbrains.plugins.addPlugins jetbrains.goland [ "github-copilot" ])
    go
  ];
}
