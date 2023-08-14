{ pkgs, ... }:
{
  programs.steam.enable = true;
  programs.gamemode.enable = true;
  environment.systemPackages = [
    pkgs.gamescope
  ];
}
