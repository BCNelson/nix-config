{ inputs, config, lib, modulesPath, pkgs, ... }:
{
  imports =
    [
      (modulesPath + "/installer/scan/not-detected.nix")
      inputs.nixos-hardware.nixosModules.common-cpu-amd
    ];

  boot.initrd.availableKernelModules = [ "xhci_pci" "ahci" "mpt3sas" "nvme" "usbhid" "usb_storage" "sd_mod" ];
  boot.initrd.kernelModules = [ "i915" "xe" ];  # Load both drivers, i915 first for A380
  boot.kernelModules = [ "kvm-amd" ];
  boot.extraModulePackages = [ ];

  fileSystems."/" =
    {
      device = "/dev/disk/by-uuid/a8500844-3dc2-4ac0-b443-adf3d75b512c";
      fsType = "ext4";
    };

  fileSystems."/boot" =
    {
      device = "/dev/disk/by-uuid/6D58-A60D";
      fsType = "vfat";
    };

  boot = {
    kernelPackages = pkgs.linuxPackages_6_12;
    zfs = {
      extraPools = [ "vault" "scary" ];
      forceImportRoot = false;
      devNodes = "/dev/disk/by-partlabel";
    };
  };

  boot.kernelParams = [
    # A380 (i915) configuration
    "i915.enable_guc=3"
    "i915.force_probe=56a5"
    
    # AMD IOMMU fix for B580
    "iommu=pt"
    "pcie_aspm=off"
  ];

  services.zfs.autoScrub.enable = true;

  swapDevices = [ ];

  hardware.enableAllFirmware = true;
  hardware.enableRedistributableFirmware = true;
  hardware.graphics = {
    enable = true;
    extraPackages = with pkgs; [
      intel-media-driver     # VA-API (iHD) for A380 transcoding
      vpl-gpu-rt             # QuickSync Video runtime
      intel-compute-runtime  # OpenCL + Level Zero for B580 AI
      level-zero             # Level Zero API
    ];
  };

  # Consistent GPU device ordering
  environment.sessionVariables = {
    LIBVA_DRIVER_NAME = "iHD";
    ZE_ENABLE_PCI_ID_DEVICE_ORDER = "1";
    ZE_FLAT_DEVICE_HIERARCHY = "FLAT";
  };

  networking.useDHCP = lib.mkDefault true;
  networking.hostId = "aa99924f";

  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
  hardware.cpu.amd.updateMicrocode = lib.mkDefault config.hardware.enableRedistributableFirmware;
}