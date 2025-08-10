{ pkgs, outputs, ... }:

{

  imports = [
    outputs.homeModules.autostart
    ./_mixins/kde.nix
  ];

  home.packages = [
    pkgs.gcompris
  ];

  systemd.user.startServices = "sd-switch";
}
