{ pkgs, ... }:
{
  home.packages = with pkgs; [
    blender-hip
  ];
}
