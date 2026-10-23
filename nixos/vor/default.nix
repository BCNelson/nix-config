{ config, pkgs, ... }:{
  imports =
    [
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/zfs.nix
      ../_mixins/roles/server/monitoring.nix
      ../_mixins/roles/server/nginx.nix
      ../_mixins/roles/server/recovery.nix
      ./services
      ./samba.nix
      ./backups.nix
      ./dataDirs.nix
    ];

  age.secrets.ntfy_topic.rekeyFile = ../../secrets/store/ntfy_topic.age;
  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;
  age.secrets.cadence_check_auto_update_vor.rekeyFile =
    ../../secrets/store/cadence/checks/auto-update-vor.age;

  services.bcnelson.autoUpdate = {
    enable = true;
    path = "/config";
    reboot = true;
    refreshInterval = "1h";
    ntfy = {
      enable = true;
      topicFile = config.age.secrets.ntfy_topic.path;
    };
    ntfy-refresh = {
      enable = true;
      topicFile = config.age.secrets.ntfy_refresh_topic.path;
    };
    healthCheck = {
      enable = true;
      url = "https://health.b.nel.family";
      uuidFile = config.age.secrets.cadence_check_auto_update_vor.path;
    };
  };

  # md127 (vor:swap0, a leftover mirror from the 2023 install) means mdadm
  # --monitor runs, and with no mail address or alert command it exits 1. That
  # failed every boot unnoticed, then failed a switch once an mdadm bump
  # restarted the unit. Send its events to the journal (and so Loki) instead.
  # PROGRAM takes a bare path; mdadm appends the event, array and device.
  boot.swraid.mdadmConf = "PROGRAM ${pkgs.writeShellScript "mdadm-alert" ''
    exec ${pkgs.util-linux}/bin/logger -t mdadm -p daemon.warning -- "$@"
  ''}";

  networking.hostId = "d80836c3";
}
