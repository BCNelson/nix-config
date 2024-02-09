{ lib, modulesPath, ... }:
{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      (modulesPath + "/installer/sd-card/sd-image.nix")
      (modulesPath + "/installer/sd-card/sd-image-aarch64.nix")
    ];
  disabledModules = [
    "profiles/all-hardware.nix"
    "profiles/base.nix"
  ];

  boot.loader.systemd-boot.enable = lib.mkForce false;
  boot.loader.efi.canTouchEfiVariables = lib.mkForce false;
}
