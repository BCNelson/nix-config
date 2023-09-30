{ pkgs, ... }:

{
    # boot.supportedFilesystems = [ "zfs" ];
    boot.zfs = {
        enable = true;
    }
    # environment.systemPackages = [
    #     pkgs.zfs
    # ];
}