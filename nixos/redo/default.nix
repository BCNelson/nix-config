{ inputs, config, pkgs, ... }:
{
  imports = [
    inputs.lanzaboote.nixosModules.lanzaboote
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
    pkgs.sbctl
  ];

  services= {
    pcscd.enable = true;
    kmscon.enable = true;
  };

  services.bcnelson.happy-daemon = {
    enable = true;
    user = "bcnelson";
    extraPackages = with pkgs; [ claude-code codex ];
    ntfyTopicFile = config.age.secrets.happy_ntfy_topic.path;
  };

  programs.nix-ld.enable = true;
}
