{ pkgs, ... }:
{
  home.packages = with pkgs; [
    handbrake
  ];
}
