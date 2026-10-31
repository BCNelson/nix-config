{ config, pkgs, ... }:
{
  home.packages = [
    # nixGL.wrap is a no-op on NixOS and adds the GL shim on genericLinux (redo).
    (config.lib.nixGL.wrap pkgs.vlc)
  ];
}
