{ pkgs, ... }:

{
  imports =
    [
      # Include the results of the hardware scan.
      ./hardware-configuration.nix
      ../_mixins/roles/desktop
      # ../_mixins/roles/desktop/hyperland.nix
      ../_mixins/roles/gaming.nix
      ../_mixins/roles/docker.nix
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/flatpak.nix
      ../_mixins/roles/fonts.nix
      ../_mixins/roles/appimage.nix
      ../_mixins/roles/nixified-ai.nix
      ../_mixins/roles/figurine.nix
      ../_mixins/roles/emulator.nix
      ../_mixins/roles/weylus.nix
      ../_mixins/hardware/streamdeck.nix
      ../_mixins/hardware/qmk.nix
      ../_mixins/hardware/platfromio.nix
      ../_mixins/roles/nfs.nix
      ../_mixins/roles/kanidmClient.nix
    ];
  networking.firewall = {
    enable = true;
    allowedTCPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
      { from = 9090; to = 9100; } # local services
    ];
    allowedUDPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
    ];
    allowedTCPPorts = [ 22000 ]; # Syncthing
    allowedUDPPorts = [ 22000 21027 ]; # Syncthing
  };
  boot.binfmt.emulatedSystems = [ "aarch64-linux" ];
  services.printing.enable = true;
  services.printing.drivers = [ pkgs.hplipWithPlugin ];

  fileSystems."/mnt/photos" = {
    device = "romeo.b.nel.family:/export/photos";
    fsType = "nfs";
    options = [ "noauto" "x-systemd.automount" "x-systemd.idle-timeout=600" "noatime" "x-systemd.requires=tailscaled.service" ];
  };

  environment.systemPackages = [
    pkgs.qemu
    (pkgs.writeShellScriptBin "qemu-system-x86_64-uefi" ''
      ${pkgs.qemu}/bin/qemu-system-x86_64 \
        -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
        "$@"
    '')
  ];

  users.groups = {
    photos = {
      name = "photos";
      gid = 27000;
      members = [ "bcnelson" ];
    };
  };

  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/home/bcnelson/nix-config";
    reboot = false;
    refreshInterval = "15m";
  };

  zramSwap.enable = true;
}
