{ config, pkgs, ... }:

{
  boot.supportedFilesystems = [ "zfs" ];
  environment.systemPackages = [
    pkgs.zfs
  ];
  services.zfs.autoScrub.enable = true;
}
