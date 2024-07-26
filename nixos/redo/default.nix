{ ... }:
{
  imports = [
    ./hardware-configuration.nix
    ../_mixins/roles/docker.nix
  ];

  allowedUDPPortRanges = [
    { from = 1714; to = 1764; } # KDE Connect
  ];
}
