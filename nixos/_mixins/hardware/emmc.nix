_:

# Make eMMC storage visible -- to an installer running from live media, and to
# the initrd of a system installed onto it.
#
# The thing to understand here is that being *available* is not the same as
# being *loaded*, and on this hardware the difference is the whole problem.
# `boot.initrd.availableKernelModules` only places a module in the initrd and
# leaves udev to load it if some device's modalias matches. That is enough for
# ordinary SATA and NVMe. It is not enough here: nothing autoloads the eMMC host
# controller on these machines, which is why installing from the ISO needed a
# manual `modprobe sdhci_pci` before any disk appeared at all.
#
# The same gap bites the initrd of the installed system, and there it is far
# more expensive. nixos-generate-config writes the controllers it detected into
# availableKernelModules, and mmc_block arrives there too via NixOS's own
# defaults ("SD cards and internal eMMC drives" in
# nixos/modules/system/boot/kernel.nix) -- so the module list looks complete and
# nothing warns. The initrd then boots, loads none of them, waits 90 s for a
# root device that no driver is looking for, and drops to an emergency shell.
# On a thin client with no root password that shell cannot even be logged into.
#
# So these are force-loaded rather than merely shipped.

{
  # The live system: what makes the target disk visible to the installer.
  boot.kernelModules = [ "sdhci_pci" "mmc_block" ];

  # The initrd: unconditionally loaded, not left to a udev match that does not
  # happen. sdhci_pci is the controller verified on the Wyse 3040 and mmc_block
  # is what presents /dev/mmcblk0; sdhci_acpi is carried along because some
  # small-form-factor units enumerate the controller through ACPI instead, and
  # loading a module that binds nothing costs nothing next to another failed
  # boot on hardware you have to walk over to.
  boot.initrd.kernelModules = [ "sdhci_pci" "sdhci_acpi" "mmc_block" ];
}
