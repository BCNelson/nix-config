_:

{
  fileSystems = {
    "/" = {
      device = "/dev/null";
    };
  };

  virtualisation.vmVariant.virtualisation = {
    memorySize = 8192;
    cores = 8;
    qemu = {
      guestAgent.enable = true;
      options = [ "-vga qxl" ];
    };
  };
}
