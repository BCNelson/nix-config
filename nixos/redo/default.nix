{ pkgs, ... }:
{
  imports = [
    ./hardware-configuration.nix
    ../_mixins/roles/docker.nix
    ../_mixins/roles/tailscale.nix
  ];

  networking.firewall = {
    enable = true;
    allowedUDPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
    ];
  };

  environment.systemPackages = [
    pkgs.qemu
    pkgs.amazon-ecr-credential-helper
    (pkgs.writeShellScriptBin "qemu-system-x86_64-uefi" ''
      ${pkgs.qemu}/bin/qemu-system-x86_64 \
        -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
        "$@"
    '')
  ];
}
