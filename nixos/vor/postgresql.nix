{ config, ... }:
let
  # immich is the only database, and its photos and nightly dumps are level2,
  # so the cluster goes there: a dedicated dataset, liveData/level2/postgresql
  # (recordsize=16K, atime=off, xattr=sa), created by hand before the move.
  # sanoid's "common" template is recursive, so the level2 policy snapshots
  # it, and romeo's pull of liveData/level2 is recursive (romeo/backups.nix).
  level2 = config.data.dirs.level2;
in
{
  # Moving the cluster from /var/lib/postgresql/14 to level2 as PG 18; see
  # modules/nixos/postgresqlLevel. Until the per-host switch commit lands,
  # dataDir is still the old cluster and the guard is a no-op.
  #
  # Before --run: DROP EXTENSION vectors in the immich database. It is the
  # pgvecto.rs leftover from immich's move to VectorChord (both vector
  # indexes are vchordrq) and has no build for PG 18, so pg_upgrade would
  # refuse the cluster.
  services.bcnelson.postgresqlLevel = {
    enable = true;
    zfs = true;
    upgrade = {
      enable = true;
      targetDir = "${level2}/postgresql/18";
    };
  };
}
