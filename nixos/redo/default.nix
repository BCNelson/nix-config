{ ... }:
{
  imports = [
    ./hardware-configuration.nix
    ../_mixins/roles/docker.nix
  ];

  networking.firewall = {
    enable = true;
    allowedUDPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
    ];
  };
}
