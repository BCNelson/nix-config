{ config, pkgs, ... }:

{
  imports = [
    ../_mixins/work/redo.nix
    ./_mixins/kwin-adaptive-workspaces.nix
    ./_mixins/workstation.nix
    ./_mixins/mcp/aws/support.nix
    ./_mixins/mcp/aws/cloudwatch.nix
    ./_mixins/mcp/datadog.nix
  ];

  home.packages = [
    (config.lib.nixGL.wrap pkgs.winbox4)
    pkgs.gam # Google Workkspace CLI
    (config.lib.nixGL.wrap pkgs.devpod-desktop)
    pkgs.coder
  ];

  programs.plasma = {
    enable = true;
    kwin.virtualDesktops = {
      names = [ "Left" "Main" "Right" ];
      rows = 1;
    };
  };
}
