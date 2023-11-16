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
    ];

  environment.systemPackages = [
    pkgs.zfs
    services.networkBacked
    pkgs.gparted
  ];

  services.unbound = {
    enable = true;
    settings = {
      interface = [ "0.0.0.0@53" "::@53" ];
      do-ipv6 = true;
      access-control = [
        "127.0.0.1/32 allow"
        "192.168.0.0/16 allow"
        "172.16.0.0.0/12 allow"
        "10.0.0.0/8 allow"
        "fc00::/7 allow"
        "::1/128 allow"
      ]
        }
        };
    }
