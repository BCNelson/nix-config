{ pkgs, ... }:
{
  imports = [
    ./nvim.nix
  ];
  
  home.packages = [
    pkgs.winbox4
  ];
}
