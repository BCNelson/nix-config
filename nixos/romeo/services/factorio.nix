{
  config,
  lib,
  ...
}:
# Factorio headless server, reachable from the public internet on UDP 34197.
#
# Space Age needs no special handling here, which is worth writing down because
# the obvious assumption is the opposite one. Since 2.0 the *free* headless
# tarball ships the expansion's data directories (space-age, quality,
# elevated-rails) next to base, and Factorio enables all three by default when
# it generates a map. So this is plain pkgs.factorio-headless: no factorio.com
# build token, no `expansion` release override, and no services.factorio.mods
# entries. Verified against factorio-headless 2.0.77 -- `--create` logs
# "Loading mod space-age 2.0.77" and writes a mod-list.json with
# elevated-rails, quality and space-age all enabled.
#
# Note this is only true of the *server*. Owning the DLC still governs whether a
# given player may join a Space Age game; the server itself does not check it.
#
# Reaching it from outside needs one change that does not live in this repo:
# a UDP 34197 port-forward to 192.168.3.7 on the router. openFirewall below only
# opens romeo's own firewall, which covers the LAN and the tailnet.
let
  dataDirs = config.data.dirs;

  # Savegames are valuable but regenerable, and they are large and fully
  # rewritten on every autosave -- a poor fit for the level3 borg repo, which
  # would store a new copy of the whole world every ten minutes. level4 gets the
  # same sanoid schedule as level3 (72 hourly / 31 daily / 24 weekly / 12
  # monthly) but no offsite copy: the level4 borg job in ../backups.nix is
  # commented out. Local snapshots are the recovery story for this one.
  stateDir = "${dataDirs.level4}/factorio";
in {
  ##########################################################################
  # Secrets
  ##########################################################################
  age.secrets.factorio-game-password = {
    rekeyFile = ./secrets/factorio_game_password.age;
    # xkcdpass, six words separated by spaces. Factorio's password field takes
    # spaces, and a word list is far easier to read out to someone than pwgen
    # output. Contains no quotes or backslashes, so it is safe to interpolate
    # into the JSON below unescaped.
    generator.script = "passphrase";

    # Synced to Bitwarden by `just sync-secrets`, because this is the one secret
    # here that a human has to read back and relay to someone else. No `username`
    # field, so it lands as a secure note rather than a login item -- a Factorio
    # server has no user half to the credential, and no URI to match against
    # either, since players reach it over UDP rather than in a browser.
    bitwarden = {
      name = "Factorio (romeo) - Server Password";
      notes = "Game password for the Space Age server on romeo, UDP 34197. The whitelist is open, so this password is what gates access.";
    };
  };

  # The module's preStart jq-merges this file over its generated settings, so it
  # must be a JSON fragment using the *server-settings* key names. That is
  # `game_password` with an underscore -- the `extraSettingsFile` option
  # documentation shows "game-password" with a hyphen, which silently does
  # nothing, because the key the module emits is game_password.
  age-template.files.factorio-server-settings = {
    vars.gamePassword = config.age.secrets.factorio-game-password.path;
    content = ''
      {"game_password": "$gamePassword"}
    '';
    # preStart runs under User=factorio (set below), so unlike the root-owned
    # EnvironmentFile pattern used elsewhere on this host, the service account
    # genuinely has to be able to read this one.
    owner = "factorio";
    group = "factorio";
    mode = "0400";
  };

  ##########################################################################
  # Server
  ##########################################################################
  services.factorio = {
    enable = true;

    # Opens UDP 34197 on romeo. The WAN path additionally needs the router
    # port-forward noted at the top of this file.
    openFirewall = true;

    game-name = "nel.family";
    description = "Space Age. Ask Bradley for the password.";

    # Not published to the official matching server: the game stays unlisted and
    # is joined by address. Keeping it unlisted is also what lets this file avoid
    # holding factorio.com account credentials at all -- `public = true` requires
    # them, `public = false` does not.
    public = false;
    # Broadcast on the LAN so local clients find it without typing an address.
    lan = true;

    # Two independent gates on an internet-facing port: a shared game password,
    # and a demand that the client hold a real factorio.com account. The second
    # is what makes the `admins` list below meaningful, since it ties an in-game
    # name to an account someone had to pay for.
    requireUserVerification = true;
    extraSettingsFile = config.age-template.files.factorio-server-settings.path;

    # These are factorio.com account names, not system users. requireUserVerification
    # above is what makes them trustworthy: a client cannot claim a name it has
    # not authenticated to factorio.com as.
    #
    # The module hardcodes allow_commands = "admins-only", so this list is also
    # the set of people who can run in-game commands at all.
    admins = ["bcnelson" "Maestro8"];

    # Deliberately empty: the whitelist is open, and knowing the game password is
    # what gets you in. The module only passes --use-server-whitelist when this
    # list is non-empty, so [] means "no whitelist enforced" rather than "nobody
    # allowed".
    #
    # The password is therefore the only thing standing between the internet and
    # this world, with requireUserVerification above as the second gate (a client
    # still cannot claim a factorio.com name it has not authenticated as). If
    # that ever needs tightening, listing names here closes it again -- but note
    # the list is handed to the server as a read-only /nix/store file, so
    # `/whitelist add <name>` in-game survives only until the next restart and
    # real additions have to be made here and deployed.
    allowedPlayers = [];

    autosave-interval = 10;

    # Reload the newest autosave rather than the canonical save on start, so an
    # unclean shutdown costs at most one autosave interval instead of rewinding
    # to whenever the named save was last written.
    loadLatestSave = true;

    extraSettings = {
      # The port is internet-facing, so it gets a ceiling rather than the
      # upstream default of unlimited.
      max_players = 10;
      # The level4 offsite borg job is commented out (see stateDir comment
      # above), so ZFS snapshots plus these in-game autosave slots are the only
      # rollback path. 50 covers roughly 8 hours of history at the 10-minute
      # interval below -- upstream's default of 5 is only 50 minutes.
      autosave_slots = 50;
    };
  };

  ##########################################################################
  # State on the vault
  #
  # Same three-part dance as ../services/matrix.nix, and for the same reason:
  # the module hardcodes its paths under /var/lib/<stateDirName> via
  # StateDirectory, so the path stays put and the storage underneath it moves.
  ##########################################################################

  # 1. DynamicUser has to go. With it on, StateDirectory=factorio puts the real
  #    directory at /var/lib/private/factorio and leaves /var/lib/factorio as a
  #    symlink to it, which the bind mount below would collide with. Unlike
  #    continuwuity, the factorio module declares no static account of its own,
  #    so User=/Group= need real ones -- hence the user/group here.
  users.users.factorio = {
    isSystemUser = true;
    group = "factorio";
  };
  users.groups.factorio = {};

  systemd.services.factorio = {
    serviceConfig = {
      DynamicUser = lib.mkForce false;
      User = "factorio";
      Group = "factorio";

      ##########################################################################
      # Hardening beyond upstream's baseline
      #
      # `systemd-analyze security factorio.service` scored the stock module 5.7
      # MEDIUM. Its sandboxing block dates to a single 2019 PR (nixpkgs#60670)
      # that enabled "most" of what systemd offered at the time and has not
      # been revisited since -- some of what it's missing (ProtectClock,
      # ProtectKernelLogs, ProtectProc/ProcSubset) postdates that PR by months
      # to years; the rest (CapabilityBoundingSet, SystemCallFilter,
      # RestrictSUIDSGID, ProtectHostname) already existed in May 2019 and was
      # simply never included.
      ##########################################################################

      # DynamicUser= implies both of these; turning it off above (for the bind
      # mount) silently gave them back. Nothing under this unit creates
      # setuid/setgid files or relies on IPC objects surviving a restart, so
      # restoring them costs nothing.
      RestrictSUIDSGID = true;
      RemoveIPC = true;

      # A userspace game server binding a UDP port above 1024 and writing to
      # its own state directory needs none of the ~40 capabilities in the
      # default bounding set. Confirmed live: hosting, autosaving and the
      # existing mod-list continued working with the set emptied.
      CapabilityBoundingSet = [];

      # @system-service is nixpkgs' own standard baseline for a compiled
      # network daemon: it still permits ordinary socket/file/process
      # syscalls, just not the mount/module/reboot/debug/raw-io/etc. groups
      # this service has no reason to touch. SystemCallErrorNumber makes an
      # unanticipated call fail with EPERM rather than killing the process
      # outright, so a gap in this list degrades to a logged error instead of
      # an instant crash-loop.
      SystemCallFilter = ["@system-service"];
      SystemCallErrorNumber = "EPERM";

      # None of these touch anything the service does: no clock access, no
      # kernel log reads, no hostname changes, no inspecting other processes,
      # no ABI switching.
      ProtectClock = true;
      ProtectKernelLogs = true;
      ProtectHostname = true;
      ProtectProc = "invisible";
      ProcSubset = "pid";
      PrivateUsers = true;
      LockPersonality = true;

      # Deliberately left open: PrivateNetwork and IPAddressDeny would each
      # buy a few tenths on the exposure score, but both contradict the point
      # of this unit -- a UDP server reachable by anyone on the internet who
      # has the password (see allowedPlayers above).

      ##########################################################################
      # Resource limits
      #
      # romeo has no swap, so an unbounded memory leak or a runaway world
      # doesn't degrade -- it goes straight to the system OOM killer picking a
      # victim anywhere on the host, not necessarily this unit. A cgroup
      # MemoryMax contains that risk to just this service.
      #
      # Sized against two things measured directly rather than guessed: this
      # unit's own current usage (384MB fresh-save baseline, matching public
      # reports of 300-500MB for a new save) and romeo's real headroom (17GiB
      # kernel-reported "available", not the much larger but ZFS-ARC-inflated
      # "used" figure). Public reports put even a mature, heavily-produced
      # world (dozens of players, hundreds of hours) around 2-4GB; this is a
      # small private server, so 6GB is a backstop with real margin over that
      # outlier, not a tight fit.
      ##########################################################################
      MemoryHigh = "3G";
      MemoryMax = "6G";

      # Checked against the live unit, not assumed: it runs 45 threads (a
      # GameUpdate thread, a TaskManager pool, TransferSource, and workers),
      # so this is not the single-core-bound process it's sometimes described
      # as -- a tight quota risks throttling legitimate bursts (autosave
      # compression, fast map exploration) into visible lag. romeo has 32
      # threads at a load average of ~2.6, so 1600% (16 cores, half the host)
      # only catches a genuinely pathological runaway and never touches normal
      # play.
      CPUQuota = "1600%";
    };
    # 2. Without this, a boot that races the vault would let systemd create and
    #    chown an empty /var/lib/factorio on the root pool, and Factorio would
    #    happily generate a second, hidden world there.
    unitConfig.RequiresMountsFor = "/var/lib/factorio";
  };

  # 3. A host-level bind mount rather than the unit's own BindPaths=, because
  #    BindPaths is applied inside the service's mount namespace -- after
  #    systemd has already created and chowned the StateDirectory in the host
  #    namespace. The ownership would land on the shadowed empty directory and
  #    the service would find its save directory unwritable.
  fileSystems."/var/lib/factorio" = {
    device = stateDir;
    # "none" is how a bind mount declares "no filesystem of its own"; the option
    # is mandatory and has no default.
    fsType = "none";
    options = ["bind"];
    depends = [dataDirs.level4];
  };

  # 4. RequiresMountsFor (which fileSystems.<path>.depends generates, as
  #    "After=mnt-vault-data-level4.mount") only orders this bind mount's own
  #    *startup* after the vault dataset's mount. It does nothing once both are
  #    already active: on 2026-08-24 an unrelated flake.lock bump forced a
  #    restart of zfs-import-vault.service mid-session, and the vault pool
  #    spent ~20s in "MISSING" while re-importing. This bind mount was never
  #    told to go down with it, so it silently kept its old reference -- which,
  #    once the pool was gone, resolved to the empty mountpoint directory
  #    sitting underneath on the root disk. factorio.service then restarted,
  #    found no save file at that (wrong, empty) location, and generated a
  #    fresh world, discarding hours of play that were never actually lost --
  #    just no longer reachable through this mount. (continuwuity's identical
  #    bind-mount pattern in ../services/matrix.nix hit the exact same window
  #    that night and needs this same fix.)
  #
  #    BindsTo= is the missing half: unlike Requires=, it also propagates
  #    *stop*. If mnt-vault-data-level4.mount ever goes down again, this mount
  #    goes down with it instead of quietly going stale, and RequiresMountsFor
  #    already guarantees it won't come back until the vault genuinely has.
  #
  #    This has to be layered on as a drop-in rather than written directly
  #    into fileSystems.<path>: fileSystems produces a /etc/fstab entry that
  #    systemd's own fstab-generator converts into a unit at activation time,
  #    entirely outside anything systemd.units already knows about -- so
  #    overrideStrategy = "asDropinIfExists" (the default) would see no prior
  #    definition and write a *competing* full unit instead of extending the
  #    generator's one. "asDropin" is unconditional: it always renders to
  #    var-lib-factorio.mount.d/overrides.conf, which systemd merges onto
  #    whatever already defines that unit name, regardless of where that
  #    definition came from. Confirmed by reading the render logic in
  #    nixos/lib/systemd-lib.nix directly rather than assuming.
  systemd.units."var-lib-factorio.mount" = {
    overrideStrategy = "asDropin";
    text = "[Unit]\nBindsTo=mnt-vault-data-level4.mount\n";
  };

  # The mount point has to exist on the vault before anything can bind it.
  # 0750 matches the module's UMask=0007 posture: the service account and its
  # group, nobody else.
  systemd.tmpfiles.rules = [
    "d ${stateDir} 0750 factorio factorio -"
  ];
}
