{ config, pkgs, ... }:
let
  # immich is the only database, and its photos and nightly dumps are level2,
  # so the cluster goes there: a dedicated dataset, liveData/level2/postgresql
  # (recordsize=16K, atime=off, xattr=sa), created by hand before the move.
  # sanoid's "common" template is recursive, so the level2 policy snapshots
  # it, and romeo's pull of liveData/level2 is recursive (romeo/backups.nix).
  level2 = config.data.dirs.level2;
in
{
  # PG 18 in level2, moved from /var/lib/postgresql/14 by pg-upgrade-to-level
  # (modules/nixos/postgresqlLevel) on 2026-11-20. The package is explicit so
  # the cluster no longer follows the stateVersion default. Rollback: revert
  # this switch; the PG 14 cluster was left untouched.
  #
  # Before the upgrade, `vectors` was dropped from the immich database: the
  # pgvecto.rs leftover from immich's move to VectorChord (both vector
  # indexes are vchordrq), with no build for PG 18 and nothing using it.
  services.postgresql = {
    package = pkgs.postgresql_18;
    dataDir = "${level2}/postgresql/18";
  };

  services.bcnelson.postgresqlLevel = {
    enable = true;
    zfs = true;
    upgrade = {
      enable = true;
      targetDir = "${level2}/postgresql/18";
    };
  };
}
