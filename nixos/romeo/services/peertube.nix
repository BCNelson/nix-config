{config, lib, pkgs, ...}: let
  host = "tube.nel.family";
  storageDir = "${config.data.dirs.level3}/peertube";
  backupDir = "${config.data.dirs.level2}/peertube";
  renderDevice = "/dev/dri/by-driver/i915-render";
  # Upstream logs PT_INITIAL_ROOT_PASSWORD verbatim on first startup. Patch the
  # built bundle so a managed credential never reaches journald, Loki or backups.
  peertubePackage = pkgs.runCommand "peertube-${pkgs.peertube.version}-managed-password" {
    inherit (pkgs.peertube) meta;
    passthru = { inherit (pkgs.peertube) cli nodejs version; };
  } ''
    cp -a --reflink=auto ${pkgs.peertube} "$out"
    chmod u+w "$out/dist/core/initializers" "$out/dist/core/initializers/installer.js"
    substituteInPlace "$out/dist/core/initializers/installer.js" \
      --replace-fail "logger.info('User password: ' + password);" \
        "if (!process.env.PT_INITIAL_ROOT_PASSWORD) logger.info('User password: ' + password);"
  '';
  storageNames = [
    "tmp" "tmp_persistent" "bin" "avatars" "logs" "web_videos"
    "streaming_playlists" "original_video_files" "redundancy"
    "thumbnails" "storyboards" "previews" "captions" "torrents"
    "cache" "plugins" "client_overrides" "well_known" "uploads"
  ];
in {
  age.secrets.peertube-secret = {
    rekeyFile = ../../../secrets/store/romeo/peertube_secret.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
    owner = "peertube";
  };
  age.secrets.peertube-admin-password = {
    rekeyFile = ../../../secrets/store/romeo/peertube_admin_password.age;
    # PeerTube limits passwords to 50 characters.
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 20";
    owner = "peertube";
  };
  age.secrets.peertube-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/peertube_auth_client_secret.age;
    generator.script = "alnum";
    owner = "peertube";
  };
  age-template.files.peertube-env = {
    vars.password = config.age.secrets.peertube-admin-password.path;
    content = "PT_INITIAL_ROOT_PASSWORD=$password";
  };

  services.peertube = {
    enable = true;
    package = peertubePackage;
    localDomain = host;
    listenHttp = 9001;
    listenWeb = 443;
    enableWebHttps = true;
    configureNginx = true;
    database.createLocally = true;
    redis.createLocally = true;
    dataDirs = [storageDir];
    secrets.secretsFile = config.age.secrets.peertube-secret.path;
    serviceEnvironmentFile = config.age-template.files.peertube-env.path;
    settings = {
      listen.hostname = "127.0.0.1";
      trust_proxy = ["loopback"];
      admin.email = "admin@nel.family";
      instance.name = "Nel Family PeerTube";
      signup.enabled = false;
      # No SMTP credentials are provisioned; accounts are managed by the admin.
      smtp.transport = "smtp";
      smtp.hostname = null;
      transcoding = {
        enabled = true;
        threads = 2;
        profile = "a380-vaapi";
        resolutions = {
          "360p" = true;
          "720p" = true;
          "1080p" = true;
        };
      };
      storage = lib.genAttrs storageNames
        (name: "${storageDir}/storage/${builtins.replaceStrings ["_"] ["-"] name}/");
    };
  };

  services.nginx.virtualHosts.${host} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
  };

  systemd.tmpfiles.rules = [
    "d ${storageDir} 0750 peertube peertube - -"
    "d ${storageDir}/storage 0750 peertube peertube - -"
    "d ${backupDir} 0755 root root - -"
    "d ${backupDir}/config 0700 root root - -"
  ];

  systemd.services.peertube = {
    # PeerTube 8 uses pnpm for plugin installation; the native module still
    # supplies yarn. Keep pnpm's writable data beneath the service cache.
    path = [pkgs.pnpm];
    environment = {
      XDG_DATA_HOME = "/var/cache/peertube";
      LIBVA_DRIVER_NAME = "iHD";
      LIBVA_DRIVERS_PATH = "${pkgs.intel-media-driver}/lib/dri";
    };
    serviceConfig = {
      # Expose the A380 render node, while retaining a device cgroup allowlist.
      PrivateDevices = lib.mkForce false;
      PrivateUsers = lib.mkForce false;
      DevicePolicy = "closed";
      DeviceAllow = ["${renderDevice} rw"];
      SupplementaryGroups = ["render"];
      # pnpm normalizes package ownership. With no capabilities and a fixed
      # unprivileged UID, this cannot change ownership to another user.
      SystemCallFilter = lib.mkAfter ["chown"];
    };
    after = ["zfs-mount.service"];
    unitConfig.RequiresMountsFor = [storageDir];
  };

  # Plugin settings live in PeerTube's database. Reconcile via its supported
  # API after startup; keep the local root password in sync with agenix.
  systemd.services.peertube-sso = {
    description = "Configure PeerTube Authentik login and GPU transcoding";
    wantedBy = ["multi-user.target"];
    after = ["peertube.service"];
    requires = ["peertube.service"];
    serviceConfig = {
      Type = "oneshot";
      User = "peertube";
      Group = "peertube";
      ExecStart = "${pkgs.python3}/bin/python ${./peertube-sso.py}"
        + " http://127.0.0.1:${toString config.services.peertube.listenHttp}"
        + " ${config.age.secrets.peertube-admin-password.path}"
        + " ${config.age.secrets.peertube-oauth-client-secret.path}"
        + " ${host}";
      TimeoutStartSec = "10min";
      Restart = "on-failure";
      RestartSec = "60s";
      UMask = "0077";
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      PrivateDevices = true;
      CapabilityBoundingSet = "";
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectKernelLogs = true;
      ProtectControlGroups = true;
      RestrictSUIDSGID = true;
      RestrictNamespaces = true;
      LockPersonality = true;
      MemoryDenyWriteExecute = true;
      RestrictAddressFamilies = ["AF_INET" "AF_UNIX"];
      IPAddressDeny = "any";
      IPAddressAllow = "localhost";
    };
  };

  # PostgreSQL's live cluster stays in /var/lib; portable dumps go to the vault.
  services.postgresqlBackup = {
    enable = true;
    databases = [config.services.peertube.database.name];
    location = "${backupDir}/database";
    startAt = "*-*-* 03:15:00";
  };
  systemd.services.postgresqlBackup-peertube.unitConfig.RequiresMountsFor = [backupDir];
  systemd.timers.postgresqlBackup-peertube.timerConfig.Persistent = true;

  # Include admin UI configuration as well as media and the database in backups.
  systemd.services.peertube-config-backup = {
    description = "Back up PeerTube configuration to the vault";
    path = [pkgs.gzip];
    after = ["peertube.service"];
    unitConfig.RequiresMountsFor = [backupDir];
    serviceConfig = {
      Type = "oneshot";
      UMask = "0077";
    };
    script = ''
      ${pkgs.gnutar}/bin/tar -czf ${backupDir}/config/config.tar.gz.tmp -C /var/lib/peertube config
      ${pkgs.coreutils}/bin/mv ${backupDir}/config/config.tar.gz.tmp ${backupDir}/config/config.tar.gz
    '';
    startAt = "*-*-* 03:20:00";
  };
  systemd.timers.peertube-config-backup.timerConfig.Persistent = true;
}
