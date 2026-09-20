{ config, pkgs, lib, ... }:
let
  dataDirs = config.data.dirs;

  # Holds live Kobo refresh tokens, so level3 (borg'd offsite) and 0700. kobodl
  # rewrites this file whenever it refreshes a token (kobo.py
  # __RefreshAuthentication), which rules out an agenix secret or any other
  # read-only path.
  configDir = "${dataDirs.level3}/kobodl";
  configFile = "${configDir}/kobodl.json";

  # One empty file per epub already handed to CWA. Deliberately on level3 with
  # the config rather than beside the books: if level6 is ever lost, kobodl
  # re-downloads the library (cheap, just bandwidth) but these stamps still stop
  # it from re-importing 300 books CWA already has.
  ingestStampDir = "${configDir}/ingested";

  # kobodl's --output-dir, and the only thing it consults to decide what it has
  # already fetched: with --get-all it skips any title whose target path exists
  # (actions.py:251). It checks *existence* only -- never size, never content --
  # so once a download has been filed away where it belongs, an empty file (or
  # empty directory, for an audiobook) left behind here is a complete dedup
  # record. Nothing but bookkeeping lives here, which is what keeps epubs and
  # audiobooks from having to share a directory: kobodl insists on one output
  # dir for both, so that dir holds neither.
  ledgerDir = "${dataDirs.level6}/kobodl/ledger";

  # DRM-free ebook archive. Replaceable -- re-downloadable from Kobo, and the
  # copy that gets read lives in the Calibre library on level3.
  epubDir = "${dataDirs.level6}/kobodl/epubs";

  # Kobo audiobooks, as their own audiobookshelf library. Separate from
  # media/audiobooks (libation's tree) because the two tools lay out and name
  # their output differently.
  audiobookDir = "${dataDirs.level6}/media/kobo";

  # CWA's drop-zone, created by calibre-web-automated.nix. Not created here, so
  # the two definitions cannot drift on ownership.
  ingestDir = "${dataDirs.level6}/calibre-ingest";

  # ntfy's the activation code rather than leaving it in a journal nobody is
  # watching. Deliberately a stdout filter over the supported `user add` CLI
  # rather than a small Python program importing kobodl as a library: the CLI is
  # what upstream maintains, and if the wording of its prompt ever changes the
  # only thing lost is the push -- the code is still on stderr in the journal.
  #
  # PYTHONUNBUFFERED is load-bearing. kobodl print()s the code and then blocks
  # in __WaitTillActivation (kobo.py:102, a `while True` with no timeout), and
  # Python block-buffers stdout when it is a pipe, so without this the code
  # would not reach the filter until the process exits -- which is exactly the
  # moment it stops being useful.
  auth = pkgs.writeShellScript "kobodl-auth" ''
    set -uo pipefail

    topic=$(cat "$CREDENTIALS_DIRECTORY/ntfy-topic")

    notify() {
      ${pkgs.curl}/bin/curl -fsS -m 15 \
        -H "X-Title: $1" \
        -H "X-Priority: 4" \
        -H "X-Tags: books,key" \
        -H "X-Click: https://www.kobo.com/activate" \
        -d "$2" "https://ntfy.sh/$topic" >/dev/null || true
    }

    export PYTHONUNBUFFERED=1
    ${lib.getExe pkgs.kobodl} --config ${configFile} user add 2>&1 | while IFS= read -r line; do
      # Keep the journal copy: this is the fallback if the push never lands.
      printf '%s\n' "$line"
      case "$line" in
        *kobo.com/activate*)
          code=$(printf '%s' "$line" | ${pkgs.gnused}/bin/sed -n 's/.*enter \([A-Za-z0-9]\{4,\}\).*/\1/p')
          notify "Kobo login code: ''${code:-see body}" "$line"
          ;;
      esac
    done
    rc=''${PIPESTATUS[0]}

    if [ "$rc" -ne 0 ]; then
      notify "Kobo login failed" "kobodl user add exited $rc -- see journalctl -u kobodl-auth"
      exit "$rc"
    fi

    # `user add` always appends. Re-authenticating an account that is already in
    # the list therefore leaves two entries for the same email, and `book get`
    # refuses to run at all once more than one user exists ("must provide --user
    # option"). getUser matches the *first* entry, so removing by email drops the
    # stale one and keeps the credentials that were just minted.
    for _ in 1 2 3 4 5; do
      dupe=$(${lib.getExe pkgs.kobodl} --config ${configFile} --fmt tsv user list \
        | tail -n +2 | cut -f1 | ${pkgs.gnugrep}/bin/grep -v '^$' | sort | uniq -d | head -n1)
      [ -n "$dupe" ] || break
      echo "dropping stale entry for $dupe"
      ${lib.getExe pkgs.kobodl} --config ${configFile} user rm "$dupe"
    done

    notify "Kobo account linked" "kobodl is authenticated; the next sync will pick up new purchases."
  '';

  sync = pkgs.writeShellScript "kobodl-sync" ''
    set -uo pipefail
    status=0
    shopt -s nullglob

    echo "fetching new books from Kobo"
    # --get-all walks the whole library and skips titles already on disk, so
    # this is the incremental sync; there is no "only new" flag to pass.
    #
    # That also means kobodl prints one "Skipping already downloaded book" line
    # per title in the library on every single run. Dropping them keeps a 6-hourly
    # job from flooding a journal that only holds about a day, and keeps the
    # failure notification's tail showing the failure rather than 15 lines of
    # skips. PYTHONUNBUFFERED because stdout is a pipe now, and a multi-hour run
    # whose progress only appears at exit is not much use.
    export PYTHONUNBUFFERED=1
    ${lib.getExe pkgs.kobodl} --config ${configFile} \
      book get --get-all --output-dir ${ledgerDir} 2>&1 \
      | ${pkgs.gnugrep}/bin/grep -v '^Skipping already downloaded book '
    if [ "''${PIPESTATUS[0]}" -ne 0 ]; then
      echo "kobodl exited non-zero; filing whatever did land, then failing" >&2
      status=1
    fi

    # --- file new ebooks out of the ledger ------------------------------------
    # A non-empty *.epub here is a book that arrived this run; a zero-byte one is
    # a spent dedup marker from a previous run. Every path below is on level6, so
    # the moves are renames.
    for book in ${ledgerDir}/*.epub; do
      [ -s "$book" ] || continue
      base=$(basename "$book")
      echo "archiving $base"
      if mv -- "$book" ${epubDir}/"$base"; then
        : > "$book"
      else
        # Left in the ledger, so kobodl will not re-download it and the next run
        # retries the move.
        echo "failed to archive $base" >&2
        status=1
      fi
    done

    # --- file new audiobooks out of the ledger --------------------------------
    # kobodl writes an audiobook as a directory of numbered parts (kobo.py:329).
    # A directory with anything in it is new; an empty one is a spent marker.
    for album in ${ledgerDir}/*/; do
      album=''${album%/}
      [ -n "$(${pkgs.findutils}/bin/find "$album" -mindepth 1 -maxdepth 1 -print -quit)" ] || continue
      base=$(basename "$album")
      echo "filing audiobook $base"
      if mv -- "$album" ${audiobookDir}/"$base"; then
        mkdir -p -- "$album"
      else
        echo "failed to file audiobook $base" >&2
        status=1
      fi
    done

    # --- hand un-ingested ebooks to CWA ---------------------------------------
    # Driven off the archive rather than off "what moved just now", so an ingest
    # that failed (or a CWA outage) is retried on the next run instead of being
    # silently dropped.
    for book in ${epubDir}/*.epub; do
      base=$(basename "$book")
      stamp=${ingestStampDir}/$base
      [ -e "$stamp" ] && continue

      echo "ingesting $base"
      # Copy under a .part name and rename into place. CWA watches this directory
      # and imports on sight, then deletes what it imported; upstream warns that
      # writing into it directly causes duplicate imports and database
      # corruption. A copy rather than a hardlink because CWA may convert the
      # file in place before deleting it, which a shared inode would let reach
      # back into the archive.
      tmp=${ingestDir}/$base.part
      if cp -- "$book" "$tmp" && mv -- "$tmp" ${ingestDir}/"$base"; then
        : > "$stamp"
      else
        echo "failed to ingest $base" >&2
        rm -f -- "$tmp"
        status=1
      fi
    done

    exit $status
  '';

  # Kick a run now and watch it, rather than remembering the unit name.
  kobodl-now = pkgs.writeShellScriptBin "kobodl-now" ''
    set -eu
    ${pkgs.systemd}/bin/systemctl start --no-block kobodl.service
    echo "kobodl started -- Ctrl-C stops the log, not the run"
    exec ${pkgs.systemd}/bin/journalctl -u kobodl.service -f -n 0
  '';

  # Trigger a (re-)authentication and watch it. The code is pushed to ntfy, so
  # this does not have to be run from somewhere you can read the output.
  kobodl-auth = pkgs.writeShellScriptBin "kobodl-auth" ''
    set -eu
    ${pkgs.systemd}/bin/systemctl start --no-block kobodl-auth.service
    echo "kobodl-auth started -- the login code is on its way to ntfy"
    exec ${pkgs.systemd}/bin/journalctl -u kobodl-auth.service -f -n 0
  '';

  # Ad-hoc kobodl as the service user, against the service's config:
  #
  #   kobodl-cli user list
  #   kobodl-cli book list
  kobodl-cli = pkgs.writeShellScriptBin "kobodl-cli" ''
    set -eu
    exec /run/wrappers/bin/sudo -u kobodl \
      env HOME=${configDir} \
      ${lib.getExe pkgs.kobodl} --config ${configFile} "$@"
  '';
in
{
  users.users.kobodl = {
    isSystemUser = true;
    group = "kobodl";
    home = configDir;
    description = "kobodl Kobo library downloader";
  };
  users.groups.kobodl = { };

  systemd.tmpfiles.rules = [
    "d ${configDir} 0700 kobodl kobodl -"
    # Recursive, so a config restored from backup or dropped in as root before
    # this config landed ends up owned correctly without a manual chown.
    "Z ${configDir} 0700 kobodl kobodl -"
    "d ${ingestStampDir} 0700 kobodl kobodl -"
    "d ${dataDirs.level6}/kobodl 0755 kobodl kobodl -"
    "d ${ledgerDir} 0755 kobodl kobodl -"
    "d ${epubDir} 0755 kobodl kobodl -"
    # World-readable so audiobookshelf (uid 99, gid 100) can scan it; see the
    # UMask note below.
    "d ${audiobookDir} 0755 kobodl kobodl -"
  ];

  # Mount the Kobo audiobooks as their own audiobookshelf library, alongside the
  # separate trees it already scans. The library itself still has to be added in
  # the audiobookshelf UI, pointing at /kobo.
  virtualisation.oci-containers.containers.audiobookshelf.volumes = [
    "${audiobookDir}:/kobo"
  ];

  systemd.services.kobodl = {
    description = "kobodl: download new Kobo purchases, feed ebooks to Calibre-Web-Automated";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    onFailure = [ "kobodl-notify-failure.service" ];
    serviceConfig = {
      Type = "oneshot";
      User = "kobodl";
      Group = "kobodl";
      # ingestDir is 0775 bcnelson:users; joining users is what lets the copy
      # land there without widening the drop-zone.
      SupplementaryGroups = [ "users" ];
      # 0022 so CWA (uid 1000, gid 100) and audiobookshelf (uid 99, gid 100) can
      # read what lands through the world bit. Files are group-owned kobodl, so
      # 0077 would make every book invisible to the things meant to read them --
      # same trade as libation.nix.
      UMask = "0022";
      ExecStart = sync;
      # The first run has the entire back catalogue to fetch.
      TimeoutStartSec = "6h";

      Environment = [ "HOME=${configDir}" ];

      # Hardening, mirroring libation.nix. Deliberately absent:
      #   MemoryDenyWriteExecute -- untested against CPython's dlopen of
      #     pycryptodome's compiled extensions on this host; an option that only
      #     fails on the first timer run is worse than no option.
      #   PrivateNetwork/IPAddressDeny -- the whole job talks to Kobo's CDN.
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
      # AF_NETLINK because glibc's getaddrinfo enumerates interfaces over
      # rtnetlink before it will return a usable address.
      RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" "AF_NETLINK" ];
      SystemCallArchitectures = "native";
      SystemCallFilter = [ "@system-service" "~@privileged" ];
      ReadWritePaths = [ configDir ledgerDir epubDir audiobookDir ingestDir ];
    };
  };

  # One-time (and re-)authentication. Pushes the activation code to ntfy, so the
  # bootstrap needs nothing more than tapping the notification and entering the
  # code -- no ssh session held open while kobodl polls.
  systemd.services.kobodl-auth = {
    description = "kobodl: authenticate a Kobo account, pushing the code to ntfy";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    # Fires itself on a host that has never been authenticated, so the first
    # deploy sends the code unprompted. Same gate as happy-auth-bootstrap.
    # kobodl only creates this file on its first successful Save().
    wantedBy = [ "multi-user.target" ];
    unitConfig.ConditionPathExists = "!${configFile}";
    serviceConfig = {
      Type = "oneshot";
      User = "kobodl";
      Group = "kobodl";
      Environment = [ "HOME=${configDir}" ];
      LoadCredential = "ntfy-topic:${config.age.secrets.ntfy_topic.path}";
      ExecStart = auth;
      # __WaitTillActivation polls forever; this is what bounds it. An unentered
      # code expires long before the hour is up, so a missed notification just
      # means running `kobodl-auth` again.
      TimeoutStartSec = "1h";

      NoNewPrivileges = true;
      CapabilityBoundingSet = [ "" ];
      PrivateTmp = true;
      PrivateDevices = true;
      ProtectHome = true;
      ProtectKernelModules = true;
      ProtectKernelTunables = true;
      ProtectProc = "invisible";
      ProtectSystem = "strict";
      LockPersonality = true;
      RestrictNamespaces = true;
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" "AF_NETLINK" ];
      SystemCallArchitectures = "native";
      SystemCallFilter = [ "@system-service" "~@privileged" ];
      ReadWritePaths = [ configDir ];
    };
  };

  # Without this a dead refresh token is invisible: kobodl cannot re-run the
  # interactive login by itself (the reauth hook only refreshes, kobo.py:142),
  # so the timer would just fail every 6h in a journal nobody reads.
  systemd.services.kobodl-notify-failure = {
    description = "Report a failed kobodl run to ntfy";
    serviceConfig = {
      Type = "oneshot";
      LoadCredential = "ntfy-topic:${config.age.secrets.ntfy_topic.path}";
    };
    script = ''
      topic=$(cat "$CREDENTIALS_DIRECTORY/ntfy-topic")
      # Cadence-style: keep the body small. romeo only keeps ~1 day of journal,
      # so the interesting lines are the last ones.
      # ntfy.sh rejects a message over 4KiB and these lines carry full library
      # paths, so keep the tail well under it.
      body=$(${pkgs.systemd}/bin/journalctl -u kobodl.service -n 15 --no-pager -o cat | tail -c 1500)
      ${pkgs.curl}/bin/curl -fsS -m 15 \
        -H "X-Title: kobodl sync failed on $HOSTNAME" \
        -H "X-Priority: 4" \
        -H "X-Tags: books,warning" \
        -d "$body

If this is an expired Kobo token, run: kobodl-auth" \
        "https://ntfy.sh/$topic" >/dev/null || true
    '';
  };

  systemd.timers.kobodl = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      # Offset from libation's 20min so the two library syncs do not start
      # together on every boot.
      OnBootSec = "35min";
      OnUnitActiveSec = "6h";
      # Persistent so a run missed while romeo was down is caught up on boot.
      Persistent = true;
      RandomizedDelaySec = "20min";
    };
  };

  environment.systemPackages = [ kobodl-now kobodl-auth kobodl-cli ];
}
