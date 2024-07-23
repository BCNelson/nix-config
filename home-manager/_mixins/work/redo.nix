{ pkgs, ... }:

{
  home.packages = [
    pkgs.distrobox
    pkgs.awscli2
    pkgs.slack
  ];

}