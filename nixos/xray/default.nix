{ config, lib, ... }:
{
  imports = [
    ../_mixins/roles/docker.nix
    ../_mixins/roles/gaming.nix
    ../_mixins/roles/tailscale.nix
    ../_mixins/roles/desktop
    ../_mixins/roles/emulator.nix
    ../_mixins/hardware/qmk.nix
    ../_mixins/roles/nfs.nix
  ];

  nixpkgs.config.permittedInsecurePackages = [
    "libsoup-2.74.3"
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
    options = [ "noauto" "x-systemd.automount" "x-systemd.idle-timeout=600" "noatime" "x-systemd.requires=network.target" ];
  };

  nix.settings.substituters = lib.mkBefore [ "https://nixcache.nel.family/" ];

  users.groups = {
    photos = {
      name = "photos";
      gid = 27000;
      members = [ "bcnelson" "hlnelson" ];
    };
  };

  age.secrets.cadence_check_auto_update_xray.rekeyFile =
    ../../secrets/store/cadence/checks/auto-update-xray.age;

  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/home/config";
    reboot = false;
    refreshInterval = "1h";
    healthCheck = {
      enable = true;
      url = "https://health.b.nel.family";
      uuidFile = config.age.secrets.cadence_check_auto_update_xray.path;
    };
  };
}
