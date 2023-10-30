{pkgs, ...}:

{
  home.packages = with pkgs.unstable; [
    (jetbrains.plugins.addPlugins jetbrains.rider [ "github-copilot" ])
    dotnet-sdk_7
  ];
}
