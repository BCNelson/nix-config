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

  # Subnet router for vor's LAN (the router at .1, UniFi gear, anything else
  # there) plus the exit node that used to be set by hand with `tailscale up`
  # and lived only in tailscaled's state. `--advertise-routes` replaces the
  # whole list, and the exit node is just 0.0.0.0/0 + ::/0 in that list, so
  # both flags have to be set together or one silently drops the other.
  # "server" turns on IPv4 and IPv6 forwarding; v6 was off, so exit-node
  # traffic over IPv6 never left vor. The new route still needs approving in
  # the Tailscale admin console unless an ACL autoApprover covers it.
  services.tailscale = {
    useRoutingFeatures = "server";
    extraSetFlags = [
      "--advertise-routes=192.168.138.0/24"
      "--advertise-exit-node"
    ];
  };

  networking.hostId = "d80836c3";
}
