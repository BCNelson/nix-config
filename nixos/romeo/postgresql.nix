{ config, pkgs, ... }:
let
  # tendant, gotosocial and linkwarden already dump to level2, so the cluster
  # itself goes there too: a dedicated dataset, vault/data/level2/postgresql
  # (recordsize=16K, atime=off, xattr=sa), created by hand before the move.
  # sanoid's "common" template is recursive, so the level2 policy snapshots
  # and prunes it; the level2 syncoid to vor is recursive for the same reason
  # (backups.nix).
  level2 = config.data.dirs.level2;
in
{
  # PG 18 in level2, moved from /var/lib/postgresql/14 by pg-upgrade-to-level
  # (modules/nixos/postgresqlLevel) on 2026-11-20. The package is explicit so
  # the cluster no longer follows the stateVersion default. The old PG 14
  # cluster is still in /var/lib/postgresql/14; rolling back to it means
  # reverting this and the commit that dropped the nixpkgs-pg14 pin.
  services.postgresql = {
    package = pkgs.postgresql_18;
    dataDir = "${level2}/postgresql/18";
  };

  services.bcnelson.postgresqlLevel = {
    enable = true;
    zfs = true;
  };

  # The per-app dumps in level2 are the borg backup; a running cluster's
  # files aren't. (ZFS snapshots of the dataset are fine: atomic.)
  services.borgbackup.jobs.level2.exclude = [ "${level2}/postgresql" ];
}
