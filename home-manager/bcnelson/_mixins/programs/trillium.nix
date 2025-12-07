{ pkgs, ... }:
{
  home.packages = with pkgs; [
    trilium-desktop
  ];
}