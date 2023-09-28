{ pkgs, ... }:

{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/tailscale.nix
    ];

    environment.systemPackages = [
      pkgs.zfs
    ];
}