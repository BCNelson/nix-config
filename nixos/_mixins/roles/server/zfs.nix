{ pkgs, ... }:

{
  boot.supportedFilesystems = [ "zfs" ];
  environment.systemPackages = [
    pkgs.zfs
  ];
  services.zfs.autoScrub.enable = true;

  ##########################################################################
  # Make local-fs.target actually wait for ZFS datasets to be mounted.
  #
  # Datasets with a ZFS-native mountpoint (as opposed to legacy/fstab ones)
  # are mounted by zfs-mount.service, and the .mount units that appear for
  # them are only ever *passively tracked*: they have no FragmentPath and no
  # SourcePath, an empty WantedBy and RequiredBy, and no job can start them.
  # They materialise when zfs-mount.service does its work and systemd notices
  # the result in /proc/self/mountinfo.
  #
  # zfs-mount.service already ships Before=local-fs.target, but it is only
  # WantedBy=zfs.target. systemd ordering applies solely *within a
  # transaction* -- Before= constrains the sequence of jobs that are already
  # queued together, and does nothing to pull a unit in. So unless something
  # else drags zfs-mount.service into the same transaction as local-fs.target,
  # local-fs.target does not wait for it, and whether the datasets happen to
  # be mounted in time is a race decided by how quickly the pool imports.
  #
  # Losing that race is not a graceful degradation. Every service whose state
  # lives on a dataset starts against the bare mountpoint directory on the
  # root disk, fails, and gets restarted -- five times in a few seconds, which
  # is exactly the pattern Restart= plus the default StartLimitBurst turns
  # into a permanent start-limit-hit. Those units then stay dead until someone
  # runs `systemctl reset-failed`, long after the vault has finished mounting.
  #
  # romeo's 2026-08-30 04:23 boot is the worked example: local-fs.target was
  # stopped at 04:23:22, zfs-mount.service did not finish until 04:23:56, and
  # in the 34s gap sixteen units died -- ten of them on start-limit-hit
  # (jellyfin, immich-postgres, mealie, audiobookshelf, homebox, foundryvtt,
  # fastenhealth, romm-db, actual, node-red-admin-env), taking immich-server,
  # immichframe and romm down with them as dependents. The same collapse also
  # left the ESP unmounted, so `switch-to-configuration` could not install a
  # bootloader and auto-update failed outright -- meaning the host could no
  # longer deploy the fix for its own boot problem. Nineteen hours of outage,
  # none of it visible as anything but "degraded".
  #
  # Wants= rather than Requires= is deliberate: this must make local-fs.target
  # *wait* for the mounts, not fail without them. A pool that genuinely cannot
  # be imported should still yield a bootable host that can be logged into and
  # repaired, which is precisely what a hard requirement here would take away.
  ##########################################################################
  systemd.services.zfs-mount.wantedBy = [ "local-fs.target" ];
}
