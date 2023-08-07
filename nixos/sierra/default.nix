{ ... }:

{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/desktop/common.nix
      ../_mixins/roles/desktop/kde.nix
      # ../_mixins/roles/desktop/hyperland.nix
      ../_mixins/roles/desktop/gaming.nix
      ../_mixins/roles/docker.nix
      ../_mixins/roles/tailscale.nix
    ];

  networking.hostName = "sierra-2"; # Define your hostname.
  networking.firewall = {
    enable = true;
    allowedTCPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
      { from = 9090; to = 9100; } # local services
    ];
    allowedUDPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
    ];
    allowedTCPPorts = [ 22000 ]; # Syncthing
    allowedUDPPorts = [ 22000 21027 ]; # Syncthing
  };
}