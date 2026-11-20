{ config, ... }:
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
  # Moving the cluster from /var/lib/postgresql/14 to level2 as PG 18; see
  # modules/nixos/postgresqlLevel. Until the per-host switch commit lands,
  # dataDir is still the old cluster and the guard is a no-op.
  services.bcnelson.postgresqlLevel = {
    enable = true;
    zfs = true;
    upgrade = {
      enable = true;
      targetDir = "${level2}/postgresql/18";
    };
  };

  # The per-app dumps in level2 are the borg backup; a running cluster's
  # files aren't. (ZFS snapshots of the dataset are fine: atomic.)
  services.borgbackup.jobs.level2.exclude = [ "${level2}/postgresql" ];
}
