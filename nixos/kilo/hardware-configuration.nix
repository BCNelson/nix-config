{lib,...}: {
  # Bootloader.
  boot.loader.systemd-boot.enable = lib.mkForce false;

  # Enable networking
  networking.networkmanager.enable = lib.mkForce false;
}
