{ pkgs, outputs, ... }:

{

  imports = [
    outputs.homeManagerModules.autostart
    ./_mixins/kde.nix
  ];

  home.packages = [
    pkgs.gcompris
  ];

  systemd.user.startServices = "sd-switch";
}
