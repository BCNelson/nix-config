{ pkgs, ... }:

{
  imports = [
    ../_mixins/work/redo.nix
    ./_mixins/workstation.nix
  ];

  home.packages = [
    pkgs.winbox4
  ];

  programs.plasma = {
    enable = true;
    kwin.virtualDesktops = {
      names = [ "Left" "Main" "Right" ];
      rows = 1;
    };
  };
}
