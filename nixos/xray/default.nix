{ ... }:
{
  imports = [
    ./hardware-configuration.nix
    ../_mixins/roles/docker.nix
    ../_mixins/roles/gaming.nix
    ../_mixins/roles/tailscale.nix
    ../_mixins/roles/desktop
    ../_mixins/roles/emulator.nix
    ../_mixins/hardware/qmk.nix
  ];
  networking.firewall = {
    enable = true;
    allowedTCPPortRanges = [
      { from = 9090; to = 9100; } # local services
    ];
    allowedUDPPortRanges = [
      { from = 1714; to = 1764; } # KDE Connect
    ];
    allowedTCPPorts = [ 22000 ]; # Syncthing
    allowedUDPPorts = [ 22000 21027 ]; # Syncthing
  };

  fileSystems."/mnt/photos" = {
    device = "romeo.b.nel.family:/export/photos";
    fsType = "nfs";
    options = [ "noauto" "x-systemd.automount" "x-systemd.idle-timeout=600" "noatime" ];
  };
}
