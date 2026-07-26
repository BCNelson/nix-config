{ lib, modulesPath, ... }:

# PROVISIONAL — not generated on the machine.
#
# Normally this file comes out of `nixos-generate-config` after the system is
# installed. delta-1 inverts that: it cannot evaluate this flake, so its
# closure has to be built on romeo *before* it can be installed, which means
# this file has to exist first. What is below is written from the Wyse 3040's
# spec, not scanned from the hardware.
#
# Replace it before the first install, with the machine booted on the
# installer ISO:
#
#   just thin-hwconfig delta-1 root@<iso-address>
#
# Getting `boot.initrd.availableKernelModules` wrong here produces a system
# that installs cleanly and then cannot find its root filesystem, which on a
# host with no local build capability means another publish cycle to fix.
#
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
    # The root filesystem lives on the internal eMMC. On Cherry Trail the
    # controller is ACPI-enumerated, so sdhci_acpi is the one that matters.
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
