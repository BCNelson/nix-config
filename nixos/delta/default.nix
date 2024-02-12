{ lib, modulesPath, pkgs, ... }:
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

  services.cage = {
    enable = true;
    user = "bcnelson";
    program = "${pkgs.firefox}/bin/firefox -kiosk https://dashy.h.b.nel.family";
  };
}
