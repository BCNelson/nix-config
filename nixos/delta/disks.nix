{
  # Dell Wyse 3040 (N10D): the only fixed storage is an 8 GiB eMMC module, so
  # everything here is chosen for space rather than throughput.
  #
  # This host is declared with disko rather than a generated
  # hardware-configuration.nix — not for convenience, but because it is the
  # only way the closure can exist before the disk does. Every other host in
  # this repo mounts by /dev/disk/by-uuid/…, and those UUIDs are minted by
  # mkfs during installation. delta-1 has to be *built* before it can be
  # installed, so a UUID-based config would be unresolvable at build time.
  #
  # disko derives its device paths from this declaration instead
  # (/dev/disk/by-partlabel/…, named after the attribute paths below), so they
  # are known at evaluation time and the same script that creates the
  # partitions also labels them. Nothing here depends on having seen the
  # hardware. The installer re-checks the closure's fstab against the disk it
  # just partitioned before handing over, so a mismatch surfaces then rather
  # than as an emergency shell after the first reboot.
  #
  # btrfs with zstd is the single biggest win available: the Nix store is
  # mostly text (ELF, scripts, man-less docs) and compresses to roughly half
  # its size, which is the difference between two system generations fitting
  # on this device and not. `discard=async` keeps the eMMC's FTL healthy
  # without the latency spikes of synchronous discard.
  disko.devices.disk.main = {
    type = "disk";
    # The eMMC is soldered and always enumerates as mmcblk0 on this platform.
    device = "/dev/mmcblk0";
    content = {
      type = "gpt";
      partitions = {
        ESP = {
          type = "EF00";
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

  # No on-disk swap on purpose. Swapping to eMMC is both painfully slow and
  # a wear problem; zram (see default.nix) covers the memory shortfall.
  swapDevices = [ ];
}
