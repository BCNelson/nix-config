{ pkgs, ... }:

{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
    ];

  environment.systemPackages = [
    pkgs.zfs
  ];
}
