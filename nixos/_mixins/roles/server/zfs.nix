{ pkgs, ... }:

{
  boot.supportedFilesystems = [ "zfs" ];
  environment.systemPackages = [
    pkgs.zfs
  ];
}
