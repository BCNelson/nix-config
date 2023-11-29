{ ... }:
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ./samba.nix
      ./backups.nix
    ];
  networking.hostId = "d80836c3";
}
