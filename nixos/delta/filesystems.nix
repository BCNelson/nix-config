# The counterpart to ./disks.nix, and the reason this host does not import it.
#
# delta's closure is built before its disk exists, so `fileSystems` cannot name
# anything minted at install time: not the UUIDs mkfs invents, and not a
# generated hardware-configuration.nix, which is why install-system passes
# --no-filesystems for limited hosts. Partition labels are the one identifier
# we choose ourselves — ./disks.nix writes them when it partitions, this file
# names them, and both are fixed at evaluation time.
#
# That keeps disko an install-time tool here, the same way every other host in
# this repo uses it, rather than a module in the system closure.
#
# The labels below must stay in step with ./disks.nix. `install-system
# --limited` checks that every device named here exists on the disk it just
# partitioned before it hands over, so drift fails the install rather than the
# first boot.
{
  fileSystems = {
    "/" = {
      device = "/dev/disk/by-partlabel/delta-root";
      fsType = "btrfs";
      options = [ "subvol=/root" "compress=zstd:3" "noatime" "discard=async" ];
    };
    "/nix" = {
      device = "/dev/disk/by-partlabel/delta-root";
      fsType = "btrfs";
      options = [ "subvol=/nix" "compress=zstd:3" "noatime" "discard=async" ];
    };
    "/boot" = {
      device = "/dev/disk/by-partlabel/delta-boot";
      fsType = "vfat";
      options = [ "umask=0077" ];
    };
  };

  # No on-disk swap on purpose. Swapping to eMMC is both painfully slow and a
  # wear problem; zram (see ./default.nix) covers the memory shortfall.
  swapDevices = [ ];
}
