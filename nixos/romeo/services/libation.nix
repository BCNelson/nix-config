{ config, pkgs, lib, ... }:
let
  dataDirs = config.data.dirs;

  cli = lib.getExe' pkgs.libation "libationcli";

  # Account tokens and the library database ride the daily borg job on level3;
  # everything else Libation writes (images, search index, logs) is re-derivable
  # from a scan, and the books themselves are re-downloadable from Audible.
  libationFiles = "${dataDirs.level3}/libation";
  booksDir = "${dataDirs.level6}/media/audiobooks";
  # In-progress downloads sit on the same filesystem as the finished books so
  # Libation's final move is a rename rather than a cross-device copy of a
  # multi-hundred-MB m4b. Deliberately outside media/ so audiobookshelf never
  # scans a half-written file.
  inProgressDir = "${dataDirs.level6}/libation/inprogress";

  # Settings Nix owns, merged over Settings.json on every start so a manual edit
  # or a stray GUI write cannot drift them. Keys absent here are left alone.
  #
  # `libationcli get-setting` prints every available key with its current value
  # and type. For an enum, `-o Key=BOGUS` names the backing .NET type, and the
  # members can be probed by trying candidates against that same override.
  settings = {
    Books = booksDir;
    InProgress = inProgressDir;

    # Without this the CLI treats every run as a first launch.
    FirstLaunch = false;

    # Defaults to "Ask", which on a headless box means the run blocks forever on
    # the first unplayable title. "Ignore" skips it and lets the rest of the
    # batch finish; "Abort" would throw away the whole run for one bad book.
    # Valid: Ask | Abort | Retry | Ignore.
    BadBook = "Ignore";

    # Sierra's tokens are already plaintext on disk despite the default reading
    # "Encrypted" -- the encrypted path wants an OS secret store that a systemd
    # unit has no session bus to reach. Naming it explicitly keeps romeo off the
    # "Failed to decrypt ExistingAccessToken" path that bites the Docker image.
    # Valid: Encrypted | Plaintext.
    TokenStorageMethod = "Plaintext";

    # The systemd timer below is the scan trigger; there is no GUI here to run
    # its own scan loop on top of it.
    AutoScan = false;

    # Pinned rather than left to default so a Libation upgrade changing its
    # defaults cannot silently start writing a different folder shape next to
    # the 237 directories already in booksDir. These are the values sierra has
    # been producing.
    FolderTemplate = "<title short> [<id>]";
    FileTemplate = "<title> [<id>]";
    ChapterFileTemplate = "<title> [<id>] - <ch# 0> - <ch title>";
    ChapterTitleTemplate = "<ch#> - <title short>: <ch title>";

    FileDownloadQuality = "High";
    LogLevel = "Information";

    # Without this Libation writes no log file at all, which makes its own
    # "Error processing book ... See log for more details" a dead end -- the
    # journal only ever shows that one line. The detail that actually names the
    # failure (an expired token, a decrypt error) lives here.
    #
    # Sierra's copy pins the sink hook to "Version=13.7.0.0"; omitting it keeps
    # this from breaking the next time nixpkgs bumps Libation.
    Serilog = {
      MinimumLevel = "Information";
      WriteTo = [{
        Name = "File";
        Args = {
          path = "${libationFiles}/Log.log";
          rollingInterval = "Month";
          outputTemplate =
            "{Timestamp:yyyy-MM-dd HH:mm:ss.fff} [{Level:u3}] (at {Caller}) {Message:lj}{NewLine}{Exception}";
        };
      }];
      Using = [ "Dinah.Core" "Serilog.Exceptions" ];
      Enrich = [ "WithCaller" "WithExceptionDetails" ];
    };
  };

  settingsJson = pkgs.writeText "libation-settings.json" (builtins.toJSON settings);

  # Mirrors the init half of upstream's Docker/liberate.sh, minus the parts that
  # only exist to paper over the container's copy-config-into-the-image dance.
  applySettings = pkgs.writeShellScript "libation-apply-settings" ''
    set -euo pipefail
    target="${libationFiles}/Settings.json"
    # libationcli will not create this itself -- it exits with "Cannot find
    # settings files" instead.
    [ -s "$target" ] || echo '{}' > "$target"
    # Recursive merge with the Nix-declared keys winning.
    ${lib.getExe pkgs.jq} -s '.[0] * .[1]' "$target" ${settingsJson} > "$target.tmp"
    mv "$target.tmp" "$target"
  '';

  liberate = pkgs.writeShellScript "libation-liberate" ''
    set -uo pipefail
    echo "scanning accounts"
    if ! ${cli} scan; then
      echo "scan failed; skipping liberate so an auth failure cannot look like an empty library" >&2
      exit 1
    fi
    echo "liberating books"
    exec ${cli} liberate
  '';

  # Kick a run now and watch it, rather than remembering the unit name.
  libation-now = pkgs.writeShellScriptBin "libation-now" ''
    set -eu
    ${pkgs.systemd}/bin/systemctl start --no-block libation.service
    echo "libation started -- Ctrl-C stops the log, not the run"
    exec ${pkgs.systemd}/bin/journalctl -u libation.service -f -n 0
  '';

  # Ad-hoc libationcli as the service user, with the state dir already wired up:
  #   libation-cli list-accounts
  #   libation-cli liberate --id B017V4IM1G
  #   libation-cli set-status
  libation-cli = pkgs.writeShellScriptBin "libation-cli" ''
    set -eu
    # Seed Settings.json first. libationcli refuses to run without it ("Cannot
    # find settings files") and will not create it itself, so otherwise every
    # ad-hoc command -- including the login-external needed to bootstrap the
    # account -- fails on a host where the service has not run yet.
    /run/wrappers/bin/sudo -u libation ${applySettings}
    exec /run/wrappers/bin/sudo -u libation \
      env LIBATION_FILES_DIR=${libationFiles} HOME=${libationFiles} \
      ${cli} "$@"
  '';
in
{
  users.users.libation = {
    isSystemUser = true;
    group = "libation";
    home = libationFiles;
    description = "Libation audiobook liberator";
  };
  users.groups.libation = { };

  systemd.tmpfiles.rules = [
    # 0700: AccountsSettings.json holds live Amazon refresh tokens and session
    # cookies in plaintext (see TokenStorageMethod above).
    "d ${libationFiles} 0700 libation libation -"
    # Recursive, so state restored from a backup or migrated in as root (the
    # libation user does not exist until this config lands) ends up owned
    # correctly without a manual chown.
    "Z ${libationFiles} 0700 libation libation -"
    "d ${dataDirs.level6}/libation 0755 libation libation -"
    "d ${inProgressDir} 0755 libation libation -"
  ];

  systemd.services.libation = {
    description = "Libation: scan Audible library and liberate new titles";
    environment = {
      LIBATION_FILES_DIR = libationFiles;
      HOME = libationFiles;
      DOTNET_CLI_TELEMETRY_OPTOUT = "1";
    };
    serviceConfig = {
      Type = "oneshot";
      User = "libation";
      Group = "libation";
      # booksDir is bcnelson:media 0777; joining media keeps new files readable
      # by audiobookshelf without widening anything.
      SupplementaryGroups = [ "media" ];
      # systemd-analyze flags this as world-readable and it stays anyway: new
      # files are group-owned by libation, so audiobookshelf (uid 99, gid 100)
      # can only read them via the world bit. 0077 would score better and make
      # every liberated book unreadable to the thing that plays it.
      UMask = "0022";
      ExecStartPre = applySettings;
      ExecStart = liberate;
      # The first run after the migration has 175 un-liberated titles to fetch.
      TimeoutStartSec = "12h";

      # Hardening. Scored with `systemd-analyze security` and each option
      # exercised against a real libationcli scan under systemd-run before
      # landing, because a sandbox that only fails on the first timer run is
      # worse than no sandbox.
      #
      # Deliberately absent:
      #   MemoryDenyWriteExecute -- the .NET JIT maps W+X and dies without it.
      #   ProcSubset=pid         -- .NET sizes the GC heap off /proc/meminfo.
      #   PrivateNetwork         -- the whole job is talking to Audible.
      #   IPAddressDeny          -- Audible/CDN egress has no stable IP set.
      NoNewPrivileges = true;
      CapabilityBoundingSet = [ "" ];
      AmbientCapabilities = [ "" ];
      PrivateTmp = true;
      PrivateDevices = true;
      ProtectClock = true;
      ProtectControlGroups = true;
      ProtectHome = true;
      ProtectHostname = true;
      ProtectKernelLogs = true;
      ProtectKernelModules = true;
      ProtectKernelTunables = true;
      ProtectProc = "invisible";
      ProtectSystem = "strict";
      RemoveIPC = true;
      LockPersonality = true;
      RestrictNamespaces = true;
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      # AF_NETLINK because .NET enumerates interfaces via rtnetlink on startup.
      RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" "AF_NETLINK" ];
      SystemCallArchitectures = "native";
      SystemCallFilter = [ "@system-service" "~@privileged" ];
      ReadWritePaths = [ libationFiles booksDir inProgressDir ];
    };
  };

  systemd.timers.libation = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnBootSec = "20min";
      OnUnitActiveSec = "6h";
      # Persistent so a run missed while romeo was down is caught up on boot,
      # which the container's `sleep` loop silently drops.
      Persistent = true;
      RandomizedDelaySec = "20min";
    };
  };

  environment.systemPackages = [ libation-now libation-cli ];
}
