{ ... }:

{
  imports = [
    ../_mixins/work/redo.nix
  ];

  home.packages = [
  ];

  programs.plasma = {
    enable = true;
    kwin.virtualDesktops = {
      names = [ "Left" "Main" "Right" ];
      rows = 1;
    };
  };
}
