{ pkgs, ... }:

{
  programs.plasma.configFile = {
    "kwinrc"."Plugins"."adaptive-workspacesEnabled" = true;
  };

  home.packages = [
    pkgs.kwin-adaptive-workspaces
  ];

  xdg.dataFile."kwin/scripts/adaptive-workspaces".source =
    "${pkgs.kwin-adaptive-workspaces}/share/kwin/scripts/adaptive-workspaces";
}
