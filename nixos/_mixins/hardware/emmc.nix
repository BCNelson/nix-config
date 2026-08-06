{ ... }:

# Make eMMC storage visible to an installer running from live media.
#
# Nothing loads the eMMC host controller on the installer ISOs. The live image
# boots from USB and never mounts mmcblk itself, so udev is never prompted to
# bring the controller up, and the symptom is an absence rather than an error:
# `install-system` offers no eligible disk, which reads as a dead eMMC rather
# than a missing driver. On the Dell thin clients, whose only storage is eMMC,
# that is the whole install.
#
# This is only about the *live* system. The initrd of an installed host needs no
# help here: mmc_block is already in NixOS's default initrd set (see "SD cards
# and internal eMMC drives" in nixos/modules/system/boot/kernel.nix), and
# nixos-generate-config picks the host controller up by walking the hardware
# buses -- which now works unprompted, because these modules are loaded by the
# time the installer runs.

{
  # sdhci_pci brings sdhci and mmc_core with it; mmc_block is what presents
  # /dev/mmcblk0. Neither is autoloaded here, so both are named.
  boot.kernelModules = [ "sdhci_pci" "mmc_block" ];
}
