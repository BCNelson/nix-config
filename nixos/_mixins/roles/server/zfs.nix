{ config, pkgs, ... }:

{
  boot.supportedFilesystems = [ "zfs" ];
  boot.kernelPackages = config.boot.zfs.package.latestCompatibleLinuxPackages;
  environment.systemPackages = [
    pkgs.zfs
  ];
  services.zfs.autoScrub.enable = true;
}
