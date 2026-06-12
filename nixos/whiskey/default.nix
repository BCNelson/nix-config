{ config, lib, ... }: {
  imports =
    [
      ../_mixins/roles/tailscale.nix
      ../_mixins/roles/server
      ../_mixins/roles/server/nginx.nix
      ./backup.nix
      ./services/cadence.nix
      ./services/forgejo.nix
      ./services/healthchecks.nix
      ./services/vaultwarden.nix
      ./services/kanidm.nix
      ./services/authentik.nix
      ./services/monitoring.nix
    ];

  networking.firewall.allowedTCPPorts = [ 80 443 ];

  zramSwap.enable = true;

  # Disk-backed swap as a backstop beneath zram. zram has priority 5, so the
  # kernel fills compressed-RAM swap first and only spills to this file under
  # real memory pressure. NixOS creates /swapfile on the ext4 root at activation.
  swapDevices = [{
    device = "/swapfile";
    size = 8192; # MiB = 8 GiB
  }];

  # Tiny disk (78G) + hourly auto-update rebuilds were piling up 30+ system
  # generations (~69G of /nix). Keep only the current generation plus one
  # rollback. nix-collect-garbage can't do count-based retention, so prune the
  # system profile to the last 2 generations, then let the daily GC reap the
  # now-unreferenced paths.
  nix.gc.options = lib.mkForce ""; # generation retention handled by prune service below

  # whiskey is a headless server, not a dev box: it has no nix-direnv shells to
  # protect, so it doesn't need build-time deps kept rooted. common.nix enables
  # these (helpful on workstations); here they pinned ~64k .drv + their outputs
  # (the authentik-from-source toolchain), which `nix-collect-garbage -d` can't
  # reclaim. Dropping them lets GC collect the build closure.
  nix.settings.keep-outputs = lib.mkForce false;
  nix.settings.keep-derivations = lib.mkForce false;

  systemd.services.prune-system-generations = {
    description = "Keep only the last 2 system generations (current + 1 rollback)";
    requiredBy = [ "nix-gc.service" ];
    before = [ "nix-gc.service" ];
    serviceConfig.Type = "oneshot";
    script = "${config.nix.package}/bin/nix-env -p /nix/var/nix/profiles/system --delete-generations +2";
  };

  # home-manager writes a new user-profile generation on every system rebuild,
  # into bcnelson's home (~/.local/state/nix/profiles) where the root-run
  # nix-gc can't reach it — so they pile up (138 and counting). Prune them as
  # the user: nix-collect-garbage auto-discovers the user's profiles and -d
  # keeps only the current generation of each. Runs as a system service (not
  # systemd.user) so it works on this headless box without login/linger.
  systemd.services.prune-user-generations = {
    description = "Expire old home-manager/user generations for bcnelson";
    requiredBy = [ "nix-gc.service" ];
    before = [ "nix-gc.service" ];
    environment.HOME = "/home/bcnelson";
    serviceConfig = {
      Type = "oneshot";
      User = "bcnelson";
    };
    script = "${config.nix.package}/bin/nix-collect-garbage -d";
  };

  networking.hostId = "9a637b7f";

  age.secrets.ntfy_topic.rekeyFile = ../../secrets/store/ntfy_topic.age;
  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;

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
      uuidFile = config.age.secrets.cadence_check_auto_update_whiskey.path;
    };
  };
}
