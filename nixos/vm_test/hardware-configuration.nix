{ ... }:

{
  fileSystems = {
    "/" = {
      device = "/dev/null";
    };
  };

  virtualisation.vmVariantWithBootLoader.virtualisation = {
    memorySize = 2048;
    cores = 4;
    useBootLoader = true;
    qemu = {
      guestAgent.enable = true;
    };
    useEFIBoot = true;
  };
}
