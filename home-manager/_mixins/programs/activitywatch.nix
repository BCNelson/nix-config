{ pkgs, ... }:

{
  home.packages = with pkgs.unstable; [
    activitywatch
  ];
}
