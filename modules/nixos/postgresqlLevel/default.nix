{ config, lib, pkgs, ... }:

# Keeps a host's PostgreSQL cluster where its storage level says it belongs,
# and carries the one-off tooling to move an existing cluster there across a
# major-version upgrade.
#
# Guard (always on with `enable`): the nixpkgs module runs `initdb` whenever
# dataDir has no PG_VERSION. On a host whose cluster holds real data that is
# never what you want -- a dataDir that points at a not-yet-upgraded path, or
# at a ZFS dataset that failed to mount, would come up as a fresh empty
# cluster and every app on it would "first-run" against nothing (authentik
# re-bootstrapping, immich forgetting every photo). So refuse to start
# instead, loudly, unless `allowInitdb` says a fresh cluster is intended.
#
# Upgrade (`upgrade.enable`): installs `pg-upgrade-to-level`, which runs
# pg_upgrade (copy mode) from `upgrade.oldDataDir` into `upgrade.targetDir`
# with the same extensions and preload libraries the host's config uses, then
# turns on data checksums on the new cluster. The old cluster is left exactly
# as it was, so rolling back is reverting the commit that pointed dataDir at
# the new cluster. Remove `upgrade` (and the nixpkgs-pg14 pin) once done.
let
  cfg = config.services.bcnelson.postgresqlLevel;
  pg = config.services.postgresql;
  up = cfg.upgrade;

  preload = pg.settings.shared_preload_libraries or null;
  preloadStr =
    if preload == null then ""
    else if builtins.isList preload then lib.concatStringsSep "," preload
    else toString preload;

  # Same extension set as the running config, built for each major. The old
  # one has to match what the cluster was actually using (pg_upgrade loads
  # every library the old databases reference into the new server).
  oldPackage = up.oldPackage.withPackages pg.extensions;
  newPackage = up.newPackage.withPackages pg.extensions;

  upgradeScript = pkgs.writeShellApplication {
    name = "pg-upgrade-to-level";
    runtimeInputs = with pkgs; [ coreutils util-linux systemd ];
    text = ''
      usage() {
        echo "usage: pg-upgrade-to-level --check | --run" >&2
        echo "  --check  pg_upgrade --check against the (running) old cluster" >&2
        echo "  --run    upgrade for real; postgresql.service must be stopped" >&2
        exit 2
      }
      die() { echo "pg-upgrade-to-level: $*" >&2; exit 1; }

      [ "$#" -eq 1 ] || usage
      case "$1" in
        --check) mode=check ;;
        --run) mode=run ;;
        *) usage ;;
      esac
      [ "$(id -u)" -eq 0 ] || die "run as root (sudo)"

      OLD_DATA=${lib.escapeShellArg up.oldDataDir}
      NEW_DATA=${lib.escapeShellArg up.targetDir}
      OLD_BIN=${oldPackage}/bin
      NEW_BIN=${newPackage}/bin
      PRELOAD=${lib.escapeShellArg preloadStr}
      PARENT=$(dirname "$NEW_DATA")
      DONE_MARKER="$NEW_DATA.upgraded"

      as_pg() {
        runuser -u postgres -- env \
          LOCALE_ARCHIVE=${config.i18n.glibcLocales}/lib/locale/locale-archive \
          "$@"
      }

      [ -f "$OLD_DATA/PG_VERSION" ] || die "no old cluster at $OLD_DATA"
      [ "$(cat "$OLD_DATA/PG_VERSION")" = ${lib.escapeShellArg (lib.versions.major up.oldPackage.version)} ] \
        || die "$OLD_DATA is not a PostgreSQL ${lib.versions.major up.oldPackage.version} cluster"
      [ "$OLD_DATA" != "$NEW_DATA" ] || die "old and new data dirs are the same"
      [ ! -e "$DONE_MARKER" ] || die "$NEW_DATA was already upgraded ($DONE_MARKER exists)"
      ${lib.optionalString cfg.zfs ''
        mountpoint -q "$PARENT" \
          || die "$PARENT is not a mounted dataset; create it first (zfs create ...)"
      ''}

      if [ "$mode" = run ] && systemctl is-active --quiet postgresql.service; then
        die "postgresql.service is still running; stop the apps and postgresql first"
      fi

      # A fresh new cluster every time: --check leaves one behind, and a
      # failed --run can leave a half-written one. Nothing else lives here
      # until the done marker exists (checked above).
      # On ZFS the parent is the dataset's mountpoint (checked above); on a
      # plain level directory (whiskey) it doesn't exist yet.
      mkdir -p "$PARENT"
      chown postgres:postgres "$PARENT"
      chmod 0750 "$PARENT"
      rm -rf -- "$NEW_DATA"
      install -d -m 0700 -o postgres -g postgres "$NEW_DATA"

      # Checksums must match the old cluster for pg_upgrade (14 has them off,
      # and 18's initdb turns them on by default); they are enabled after the
      # upgrade instead, on the new cluster only.
      as_pg "$NEW_BIN/initdb" -D "$NEW_DATA" -U ${lib.escapeShellArg pg.superUser} \
        --encoding=UTF8 --locale=${lib.escapeShellArg cfg.locale} --no-data-checksums \
        ${lib.escapeShellArgs pg.initdbArgs}

      args=(
        --old-datadir "$OLD_DATA" --new-datadir "$NEW_DATA"
        --old-bindir "$OLD_BIN" --new-bindir "$NEW_BIN"
        --username ${lib.escapeShellArg pg.superUser}
        --jobs "$(nproc)"
      )
      if [ -n "$PRELOAD" ]; then
        args+=(--old-options "-c shared_preload_libraries=$PRELOAD")
        args+=(--new-options "-c shared_preload_libraries=$PRELOAD")
      fi
      if [ "$mode" = check ]; then
        args+=(--check)
      fi

      # pg_upgrade writes its logs and sockets in the cwd, which must be
      # writable by postgres.
      cd "$PARENT" || die "cannot cd to $PARENT"
      as_pg "$NEW_BIN/pg_upgrade" "''${args[@]}"

      if [ "$mode" = check ]; then
        rm -rf -- "$NEW_DATA"
        echo "pg-upgrade-to-level: check passed; nothing was changed"
        exit 0
      fi

      ${lib.optionalString up.enableChecksums ''
        as_pg "$NEW_BIN/pg_checksums" --enable --progress -D "$NEW_DATA"
      ''}
      touch "$DONE_MARKER"
      echo "pg-upgrade-to-level: upgraded $OLD_DATA -> $NEW_DATA"
      echo "next: deploy dataDir=$NEW_DATA with package ${up.newPackage.name}, then vacuumdb --all --analyze-in-stages"
    '';
  };
in
{
  options.services.bcnelson.postgresqlLevel = {
    enable = lib.mkEnableOption ''
      the empty-cluster guard: postgresql refuses to start (instead of
      running initdb) when its dataDir holds no cluster
    '';

    allowInitdb = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Let the nixpkgs module initdb an empty dataDir (a deliberately fresh cluster).";
    };

    zfs = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        dataDir lives on a ZFS-native mountpoint. Those have no fstab mount
        unit, so the module's RequiresMountsFor= doesn't order postgresql
        after them; require zfs-mount.service instead.
      '';
    };

    locale = lib.mkOption {
      type = lib.types.str;
      default = "en_US.UTF-8";
      description = "Locale for the new cluster's initdb; must match the old cluster's.";
    };

    upgrade = {
      enable = lib.mkEnableOption "the pg-upgrade-to-level script";
      oldDataDir = lib.mkOption {
        type = lib.types.str;
        default = "/var/lib/postgresql/14";
        description = "Data directory of the cluster to upgrade from.";
      };
      oldPackage = lib.mkOption {
        type = lib.types.package;
        default = pkgs.postgresql_14;
        defaultText = lib.literalExpression "pkgs.postgresql_14";
        description = "PostgreSQL package (without extensions) of the old cluster.";
      };
      newPackage = lib.mkOption {
        type = lib.types.package;
        default = pkgs.postgresql_18;
        defaultText = lib.literalExpression "pkgs.postgresql_18";
        description = "PostgreSQL package (without extensions) to upgrade to.";
      };
      targetDir = lib.mkOption {
        type = lib.types.str;
        example = "/mnt/vault/data/level2/postgresql/18";
        description = "Data directory of the new cluster, under the host's storage level.";
      };
      enableChecksums = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Run pg_checksums --enable on the new cluster after pg_upgrade.";
      };
    };
  };

  config = lib.mkIf (cfg.enable && pg.enable) {
    systemd.services.postgresql = {
      preStart = lib.mkIf (!cfg.allowInitdb) (lib.mkBefore ''
        if ! test -e ${pg.dataDir}/PG_VERSION; then
          echo "refusing to initdb an empty cluster at ${pg.dataDir}." >&2
          echo "Either the cluster hasn't been moved/upgraded there yet (pg-upgrade-to-level)," >&2
          echo "or its filesystem didn't mount. Set services.bcnelson.postgresqlLevel.allowInitdb" >&2
          echo "only if a fresh, empty cluster is really what you want." >&2
          exit 1
        fi
      '');
      requires = lib.mkIf cfg.zfs [ "zfs-mount.service" ];
      after = lib.mkIf cfg.zfs [ "zfs-mount.service" ];
    };

    environment.systemPackages = lib.mkIf up.enable [ upgradeScript ];
  };
}
