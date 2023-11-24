{ pkgs, libx, ... }:
let
  dataDirs = {
    level1 = "/mnt/data/level1"; # Critical
    level2 = "/mnt/data/level2"; # Important
    level3 = "/mnt/data/level3"; # High
    level4 = "/mnt/data/level4"; # Medium
    level5 = "/mnt/data/level5"; # Low
    level6 = "/data/replaceable"; # Replaceable
    level7 = "/cache"; # Ephemeral
  };
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
    ];
  networking.hostId = "d80836c3";
}
