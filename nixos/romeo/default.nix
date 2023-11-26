{ pkgs, libx, ... }:
let
  dataDirs = {
    level1 = "/mnt/vault/data/level1"; # Critical
    level2 = "/mnt/vault/data/level2"; # Important
    level3 = "/mnt/vault/data/level3"; # High
    level4 = "/mnt/vault/data/level4"; # Medium
    level5 = "/mnt/vault/data/level5"; # Low
    level6 = "/data/replaceable"; # Replaceable
    level7 = "/cache"; # Ephemeral
  };
  services = import ./services.nix { inherit libx dataDirs pkgs; };
in
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ./unbound.nix
      ./backup.nix
    ];

  environment.systemPackages = [
    pkgs.zfs
    services.networkBacked
    pkgs.gparted
  ];
}
