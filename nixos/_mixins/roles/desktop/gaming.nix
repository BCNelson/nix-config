{pkgs, ...}:
{
  programs.steam.enable = true;
  environment.systemPackages = [
    pkgs.gamescope
  ];
}
