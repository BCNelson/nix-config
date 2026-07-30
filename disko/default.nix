{ disk, swapSize ? "0G", ... }:
let
  # install-system always passes --arg swapSize, sending "0G" when the user
  # picks "None". This file used to take `{ disk, ... }`, so that argument was
  # swallowed by the ellipsis and no swap partition was ever created on the
  # unencrypted path -- the installer asked the question and then ignored the
  # answer. disko/luks.nix always honoured it.
  wantSwap = swapSize != "0G" && swapSize != "0" && swapSize != "";

  swapPartition =
    if wantSwap then {
      swap = {
        size = swapSize;
        content = {
          type = "swap";
          discardPolicy = "both";
        };
      };
    } else { };
in
{
  disko.devices = {
    disk = {
      main = {
        device = disk;
        type = "disk";
        content = {
          type = "gpt";
          # Ordering needs no explicit priorities: disko gives a partition with
          # `size = "100%"` priority 9001, so root is created last, after swap
          # has taken its fixed slice.
          partitions = {
            ESP = {
              type = "EF00";
              size = "1G";
              content = {
                type = "filesystem";
                format = "vfat";
                mountpoint = "/boot";
              };
            };
          } // swapPartition // {
            root = {
              size = "100%";
              content = {
                type = "filesystem";
                format = "ext4";
                mountpoint = "/";
              };
            };
          };
        };
      };
    };
  };
}
