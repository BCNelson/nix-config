{ pkgs, ... }:

{
  home.packages = [
    pkgs.distrobox
    pkgs.awscli2
  ];

}