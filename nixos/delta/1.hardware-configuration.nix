{ lib, modulesPath, ... }:

# Dell Wyse 3040 (N10D001): Intel Atom x5-Z8350 (Cherry Trail), 2 GiB DDR3L,
# 8 GiB eMMC, Realtek gigabit ethernet, 64-bit UEFI.
#
# Filesystems come from ./disks.nix (disko), so this only covers the modules
# needed to reach that storage and the platform defaults.

{
  imports = [ (modulesPath + "/installer/scan/not-detected.nix") ];

  boot.initrd.availableKernelModules = [
    "xhci_pci"
    "usbhid"
    "usb_storage"
    "sd_mod"
    # The root filesystem lives on the internal eMMC.
    "sdhci_pci"
    "sdhci_acpi"
    "mmc_block"
  ];
  boot.initrd.kernelModules = [ ];
  boot.kernelModules = [ ];
  boot.extraModulePackages = [ ];

  hardware.cpu.intel.updateMicrocode = lib.mkDefault true;

  networking.useDHCP = lib.mkDefault true;
}
