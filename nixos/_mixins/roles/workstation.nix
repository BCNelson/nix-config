{pkgs, ...}:
{
  services= {
    pcscd.enable = true;
    kmscon.enable = true;
  };
  programs.partition-manager.enable = true;

  boot.kernelPackages = pkgs.linuxPackages_zen;
}
