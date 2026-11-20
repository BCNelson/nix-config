{ pkgs, ... }:
let
  # authentik is the login for everything, so its database is level1 (with
  # vaultwarden). /data/level1 is a plain directory on the root disk here:
  # being under it is what puts the dumps in the level1 borg job.
  level1 = "/data/level1";
in
{
  # PG 18 in level1, moved from /var/lib/postgresql/14 by pg-upgrade-to-level
  # (modules/nixos/postgresqlLevel) on 2026-11-20. The package is explicit so
  # the cluster no longer follows the stateVersion default. The old PG 14
  # cluster is still in /var/lib/postgresql/14; rolling back to it means
  # reverting this and the commit that dropped the nixpkgs-pg14 pin.
  services.postgresql = {
    package = pkgs.postgresql_18;
    dataDir = "${level1}/postgresql/18";
  };

  services.bcnelson.postgresqlLevel.enable = true;

  # authentik's database had no backup at all: borg only reads /data/level*,
  # and the cluster lived in /var/lib/postgresql. Dump it into level1 at :45
  # before each 6-hourly borg run (0/6:00), so every archive carries a dump
  # at most 15 minutes old.
  services.postgresqlBackup = {
    enable = true;
    databases = [ "authentik" ];
    location = "${level1}/postgresql-dumps";
    compression = "zstd";
    startAt = "*-*-* 05,11,17,23:45:00";
  };

  # borg copying a running cluster's files gives an archive that can't be
  # trusted to restore; the dumps above are the backup. Excluded now so the
  # first level1 run after the move doesn't sweep it in.
  services.borgbackup.jobs.level1.exclude = [ "${level1}/postgresql" ];
}
