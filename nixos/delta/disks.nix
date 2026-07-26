# Install-time only: `install-system` runs this through disko's CLI as
# `nix run disko -- nixos/delta/disks.nix --arg disk /dev/… --arg swapSize …`.
# It is NOT imported as a NixOS module — the running system gets its mounts
# from ./filesystems.nix, which names the partition labels written below.
# The ellipsis absorbs swapSize, which this host has no use for.
{ disk ? "/dev/mmcblk0", ... }:
{
  # Dell Wyse 3040 (N10D): the only fixed storage is an 8 GiB eMMC module, so
  # everything here is chosen for space rather than throughput.
  #
  # The labels below are the whole reason this host can be built before it
  # exists. Every other host here mounts by /dev/disk/by-uuid/…, and mkfs
  # mints those UUIDs during installation — too late for a closure that was
  # built days earlier. Labels are chosen by us, written here, and named by
  # ./filesystems.nix.
  #
  # btrfs with zstd is the single biggest win available: the Nix store is
  # mostly text (ELF, scripts, man-less docs) and compresses to roughly half
  # its size, which is the difference between two system generations fitting
  # on this device and not. `discard=async` keeps the eMMC's FTL healthy
  # without the latency spikes of synchronous discard.
  disko.devices.disk.main = {
    type = "disk";
    # The eMMC is soldered and always enumerates as mmcblk0; install-system
    # overrides this with whatever disk the operator picks.
    device = disk;
    content = {
      type = "gpt";
      partitions = {
        ESP = {
          type = "EF00";
          # Pinned rather than left to disko's derived "disk-main-ESP", because
          # ./filesystems.nix names it and that contract should not depend on
          # disko's internal naming.
          label = "delta-boot";
          # Three generations of kernel + initrd, with headroom for a
          # firmware update or a rescue entry.
          size = "512M";
          content = {
            type = "filesystem";
            format = "vfat";
            mountpoint = "/boot";
            mountOptions = [ "umask=0077" ];
          };
        };
        root = {
          label = "delta-root";
          size = "100%";
          content = {
            type = "btrfs";
            extraArgs = [ "-f" ];
            subvolumes = {
              "/root" = {
                mountpoint = "/";
                mountOptions = [ "compress=zstd:3" "noatime" "discard=async" ];
              };
              "/nix" = {
                mountpoint = "/nix";
                mountOptions = [ "compress=zstd:3" "noatime" "discard=async" ];
              };
            };
          };
        };
      };
    };
  };
}
