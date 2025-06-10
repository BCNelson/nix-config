{ pkgs, ... }:
{
  home.packages = [
    pkgs.prusa-slicer
    pkgs.orca-slicer
  ];
}