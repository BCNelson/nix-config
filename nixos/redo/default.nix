{ pkgs, ... }:
{
  imports = [
    ../_mixins/roles/docker.nix
    ../_mixins/roles/tailscale.nix
    ../_mixins/roles/flatpak.nix
    ../_mixins/roles/workstation.nix
  ];

  networking.firewall = {
    enable = true;
    allowedUDPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
    ];
  };

  boot.initrd.systemd.enable = true;

  environment.systemPackages = [
    pkgs.qemu
    pkgs.amazon-ecr-credential-helper
    (pkgs.writeShellScriptBin "qemu-system-x86_64-uefi" ''
      ${pkgs.qemu}/bin/qemu-system-x86_64 \
        -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
        "$@"
    '')
    pkgs.tpm2-tss
  ];

  services= {
    pcscd.enable = true;
    kmscon.enable = true;
  };
}
