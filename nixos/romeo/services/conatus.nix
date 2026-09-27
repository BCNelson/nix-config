{
  config,
  pkgs,
  ...
}:
# Conatus — self-hosted, Todoist-style task manager (pkgs/conatus).
#
# Upstream's compose stack is postgres + MinIO + the Next.js app + a one-shot
# "ops" container for migrations. Here each piece is native:
#
#   postgres  the host's shared instance, peer auth over /run/postgresql
#   RustFS    loopback-only S3, holds task attachments and avatars
#             (MinIO is marked insecure in nixpkgs and upstream is abandoned)
#   conatus   the app; migrations + first-admin bootstrap run as ExecStartPre
#
# Login is username/password only — upstream has no OIDC, so this is NOT behind
# authentik. Registration is invite-only; the first account is created by the
# bootstrap step below with a generated password, so the public signup form is
# never the way the admin comes into existence. Retrieve it with:
#   agenix decrypt secrets/store/romeo/conatus_admin_password.age  (user: bcnelson)
let
  dataDirs = config.data.dirs;

  host = "tasks.nel.family";
  port = 4399;
  s3Port = 9310;
  s3AccessKey = "conatus";

  ops = pkgs.conatus-ops;
in {
  ##########################################################################
  # Secrets
  ##########################################################################
  age.secrets.conatus-auth-secret = {
    rekeyFile = ../../../secrets/store/romeo/conatus_auth_secret.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };
  age.secrets.conatus-s3-secret-key = {
    rekeyFile = ../../../secrets/store/romeo/conatus_s3_secret_key.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };
  age.secrets.conatus-admin-password = {
    rekeyFile = ../../../secrets/store/romeo/conatus_admin_password.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 16";
  };

  # Read by systemd as root before the privilege drop, so these stay root-owned.
  age-template.files.conatus-env = {
    vars = {
      authSecret = config.age.secrets.conatus-auth-secret.path;
      s3Secret = config.age.secrets.conatus-s3-secret-key.path;
      adminPassword = config.age.secrets.conatus-admin-password.path;
    };
    content = ''
      AUTH_SECRET=$authSecret
      S3_SECRET_KEY=$s3Secret
      CONATUS_ADMIN_PASSWORD=$adminPassword
    '';
  };
  age-template.files.conatus-rustfs-credentials = {
    vars.s3Secret = config.age.secrets.conatus-s3-secret-key.path;
    content = ''
      RUSTFS_ACCESS_KEY=${s3AccessKey}
      RUSTFS_SECRET_KEY=$s3Secret
    '';
  };

  ##########################################################################
  # Backing services
  ##########################################################################
  services.postgresql = {
    ensureDatabases = ["conatus"];
    ensureUsers = [
      {
        name = "conatus";
        ensureDBOwnership = true;
      }
    ];
  };

  # RustFS is used only as conatus's attachment store; nothing else on romeo
  # talks S3. Loopback only, web console off. Attachments are user uploads
  # that cannot be recreated, so they live on level3 with the other personal
  # data (vault snapshots + borg). Conatus creates its bucket on first use.
  services.rustfs = {
    enable = true;
    settings = {
      RUSTFS_VOLUMES = "${dataDirs.level3}/conatus/rustfs";
      RUSTFS_ADDRESS = "127.0.0.1:${toString s3Port}";
      RUSTFS_CONSOLE_ENABLE = "false";
    };
    environmentFile = config.age-template.files.conatus-rustfs-credentials.path;
  };
  # Otherwise it can start before the vault mounts and initialise a fresh,
  # empty store underneath it.
  systemd.services.rustfs.unitConfig.RequiresMountsFor = ["${dataDirs.level3}/conatus"];

  ##########################################################################
  # The app
  ##########################################################################
  users.users.conatus = {
    isSystemUser = true;
    group = "conatus";
    description = "Conatus task manager";
  };
  users.groups.conatus = {};

  systemd.services.conatus = {
    description = "Conatus task manager";
    wantedBy = ["multi-user.target"];
    requires = ["postgresql.target" "rustfs.service"];
    after = ["postgresql.target" "rustfs.service" "network-online.target"];
    wants = ["network-online.target"];

    environment = {
      HOSTNAME = "127.0.0.1";
      PORT = toString port;

      # Peer auth over the socket: no DB password exists. Both postgres.js and
      # pg (pg-boss) take the socket directory from PGHOST and the role from
      # PGUSER when the URL has neither. It has to be exactly this shape:
      # "postgres://conatus@/conatus" (user, empty host) is not a valid WHATWG
      # URL and postgres.js throws on it.
      DATABASE_URL = "postgres:///conatus";
      PGHOST = "/run/postgresql";
      PGUSER = "conatus";

      AUTH_URL = "https://${host}";
      PUBLIC_BASE_URL = "https://${host}";
      REGISTRATION_MODE = "invite-only";

      S3_ENDPOINT = "127.0.0.1";
      S3_PORT = toString s3Port;
      S3_ACCESS_KEY = s3AccessKey;
      S3_BUCKET = "attachments";

      # Only acts on an empty users table; a no-op on every later start.
      CONATUS_ADMIN_USERNAME = "bcnelson";
    };

    serviceConfig = {
      # Migrations are idempotent and fast; running them on every start is what
      # upstream's compose file does too, and it means a package bump can never
      # start the new app against the old schema.
      ExecStartPre = [
        "${ops}/bin/conatus-migrate"
        "${ops}/bin/conatus-bootstrap-admin"
      ];
      ExecStart = "${pkgs.conatus}/bin/conatus";
      EnvironmentFile = config.age-template.files.conatus-env.path;
      User = "conatus";
      Group = "conatus";
      Restart = "on-failure";
      RestartSec = 5;
      CacheDirectory = "conatus";
      UMask = "0077";

      # Stateless apart from its cache: everything lives in postgres and RustFS.
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      PrivateDevices = true;
      NoNewPrivileges = true;
      CapabilityBoundingSet = [""];
      RestrictSUIDSGID = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectKernelLogs = true;
      ProtectControlGroups = true;
      ProtectClock = true;
      ProtectHostname = true;
      ProtectProc = "invisible";
      RestrictNamespaces = true;
      LockPersonality = true;
      RestrictRealtime = true;
      RemoveIPC = true;
      # Outbound HTTPS is needed for user-configured webhooks.
      RestrictAddressFamilies = ["AF_INET" "AF_INET6" "AF_UNIX"];
      # No MemoryDenyWriteExecute: V8 JITs.
      SystemCallArchitectures = "native";
      SystemCallFilter = ["@system-service" "~@privileged" "~@resources"];
      MemoryMax = "1G";
    };
  };

  ##########################################################################
  # Backups
  #
  # The live postgres data dir is under /var/lib, not the vault, so these
  # dumps are what snapshots/borg/syncoid actually capture for the DB.
  # Attachments are already on level3 via RustFS's volume.
  ##########################################################################
  systemd.tmpfiles.rules = [
    "d ${dataDirs.level3}/conatus          0755 root     root     - -"
    "d ${dataDirs.level2}/conatus          0750 postgres postgres - -"
    "d ${dataDirs.level2}/conatus/db-dumps 0750 postgres postgres - -"
  ];

  systemd.services.conatus-db-backup = {
    description = "Dump Conatus postgres DB to the vault for backup";
    after = ["postgresql.target"];
    path = [config.services.postgresql.package pkgs.coreutils pkgs.findutils];
    unitConfig.RequiresMountsFor = ["${dataDirs.level2}/conatus"];
    serviceConfig = {
      Type = "oneshot";
      User = "postgres";
      Group = "postgres";
    };
    script = ''
      set -euo pipefail
      dir="${dataDirs.level2}/conatus/db-dumps"
      pg_dump -Fc -Z 9 -f "$dir/conatus-$(date +%Y%m%d-%H%M%S).dump" conatus
      ls -1t "$dir"/conatus-*.dump | tail -n +15 | xargs -r rm -f
    '';
  };

  systemd.timers.conatus-db-backup = {
    wantedBy = ["timers.target"];
    timerConfig = {
      OnCalendar = "*-*-* 03:15:00";
      Persistent = true;
    };
  };

  ##########################################################################
  # nginx
  ##########################################################################
  services.nginx.virtualHosts."${host}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    http2 = true;
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString port}";
      proxyWebsockets = true;
      # Off on purpose: the recommended include lands after extraConfig and
      # re-adds the client's own X-Forwarded-For, and Conatus rate-limits
      # logins on the FIRST entry of that header — so anyone could dodge the
      # limit by sending one. Set it to the real peer only.
      recommendedProxySettings = false;
      extraConfig = ''
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $remote_addr;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_set_header X-Forwarded-Host $host;
        # Attachments cap at 10 MB, Todoist imports at 25 MB.
        client_max_body_size 30M;
      '';
    };
  };
}
