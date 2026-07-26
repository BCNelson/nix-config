{
  # Dell Wyse 3040 (N10D): the only fixed storage is an 8 GiB eMMC module, so
  # everything here is chosen for space rather than throughput.
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
