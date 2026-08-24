{
  config,
  lib,
  pkgs,
  ...
}:
# openGym — self-hosted gym & body-weight tracker, run natively instead of from
# upstream's compose stack. Three pieces on ONE origin: the API on loopback, the
# prebuilt SPA, and the exercise media, all fronted by a single nginx vhost.
#
# The single-origin part is not a style choice. WebAuthn binds a passkey to the
# exact hostname it was created on, so the moment /api answers somewhere other
# than the app, every passkey stops verifying.
#
# Exercised end-to-end by pkgs/opengym/nixos-test.nix — anything added here that
# can break at runtime should grow an assertion there.
let
  cfg = config.services.bcnelson.opengym;
  inherit (lib) mkIf mkOption mkEnableOption types optionalAttrs;

  scheme =
    if cfg.useACME
    then "https"
    else "http";

  boolEnv = b:
    if b
    then "1"
    else "0";
in {
  options.services.bcnelson.opengym = {
    enable = mkEnableOption "openGym gym and body-weight tracker";

    package = mkOption {
      type = types.package;
      default = pkgs.opengym-api;
      defaultText = lib.literalExpression "pkgs.opengym-api";
      description = "The openGym API package.";
    };

    webPackage = mkOption {
      type = types.package;
      default = pkgs.opengym-web;
      defaultText = lib.literalExpression "pkgs.opengym-web";
      description = "Prebuilt openGym frontend, served as static files.";
    };

    mediaPackage = mkOption {
      type = types.package;
      default = pkgs.opengym-media;
      defaultText = lib.literalExpression "pkgs.opengym-media";
      description = ''
        Exercise images and animations. Expected to contain `images/` and
        `videos/`, which are served at /img/ and /gif/ respectively.
      '';
    };

    host = mkOption {
      type = types.str;
      example = "gym.example.com";
      description = ''
        Public hostname. Used as the nginx virtual host, as the WebAuthn relying
        party id, and to build ORIGIN. Changing it after passkeys exist
        invalidates every one of them.
      '';
    };

    useACME = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Terminate TLS with an ACME certificate. Turning this off serves plain
        HTTP, which browsers only accept passkeys over for localhost — it exists
        for tests and for setups that terminate TLS further out.
      '';
    };

    port = mkOption {
      type = types.port;
      default = 3111;
      description = "Loopback port the API listens on. Never exposed directly.";
    };

    dataDir = mkOption {
      type = types.path;
      example = "/var/lib/opengym";
      description = ''
        Where accounts, passkeys, per-user workout state, the session-signing
        secret and the audit log live. Back this up: it is the whole instance.
      '';
    };

    inviteOnly = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Require an invite code to create a passkey profile. Defaults on, which is
        the opposite of upstream's default: an instance built from this module is
        assumed to be fronted by an identity provider, and an open signup form
        beside it is a way in that bypasses it.
      '';
    };

    allowGuest = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Offer "continue without account", which keeps everything in the browser
        and never touches the server. Off by default for the same reason as
        {option}`inviteOnly`.
      '';
    };

    environmentFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      description = ''
        Extra environment for the unit, read by systemd as root before the
        privilege drop. This is where OIDC_CLIENT_SECRET belongs — anything put
        in {option}`extraEnvironment` lands in the world-readable store instead.
      '';
    };

    extraEnvironment = mkOption {
      type = types.attrsOf types.str;
      default = {};
      example = {
        SESSION_DAYS = "30";
      };
      description = ''
        Additional environment variables. Goes into the nix store, so never
        secrets — see {option}`environmentFile`.
      '';
    };

    oidc = {
      enable = mkEnableOption "OpenID Connect sign-in alongside passkeys";

      issuer = mkOption {
        type = types.str;
        example = "https://auth.example.com/application/o/opengym/";
        description = ''
          Issuer URL. The discovery document is read from
          `<issuer>/.well-known/openid-configuration`.
        '';
      };

      clientId = mkOption {
        type = types.str;
        default = "opengym";
        description = "OAuth2 client id registered with the provider.";
      };

      name = mkOption {
        type = types.str;
        default = "SSO";
        description = ''
          Provider name shown on the sign-in button ("Sign in with …").
        '';
      };

      scopes = mkOption {
        type = types.listOf types.str;
        default = ["openid" "profile" "email"];
        description = ''
          Scopes to request. On authentik there is no dedicated groups scope —
          membership rides inside `profile`, so {option}`adminGroups` needs it.
        '';
      };

      adminGroups = mkOption {
        type = types.listOf types.str;
        default = [];
        example = ["service_admins"];
        description = ''
          Groups from the id_token's `groups` claim that grant the admin
          dashboard. Empty leaves admin to whatever the instance already said;
          non-empty makes this authoritative for SSO users and re-evaluates it at
          every sign-in, so removing someone from the group actually demotes them.
        '';
      };

      autoProvision = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Create a profile the first time someone the provider vouches for signs
          in. With this on, the provider's own access policy is the only gate.
        '';
      };

      linkExisting = mkOption {
        type = types.bool;
        default = true;
        description = ''
          Adopt an existing passkey profile whose display name matches instead of
          creating a second one beside it. This is what makes an instance that
          already has people on it migratable. Turn it off where two people share
          a display name.
        '';
      };
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        # The unit hides everything beside the data directory by mounting a
        # tmpfs over its parent. A dataDir directly under / would put that tmpfs
        # on the root filesystem and take /nix with it.
        assertion = builtins.dirOf cfg.dataDir != "/";
        message = ''
          services.bcnelson.opengym.dataDir must be at least two levels deep
          (got "${cfg.dataDir}"): its parent directory is replaced by a tmpfs in
          the service's mount namespace.
        '';
      }
      {
        assertion = cfg.oidc.enable -> cfg.environmentFile != null;
        message = ''
          services.bcnelson.opengym.oidc requires environmentFile to be set:
          the client secret has to reach the unit as OIDC_CLIENT_SECRET and
          must not be written into the nix store.
        '';
      }
    ];

    users.users.opengym = {
      isSystemUser = true;
      group = "opengym";
      description = "openGym API";
    };
    users.groups.opengym = {};

    systemd.tmpfiles.rules = [
      "d ${cfg.dataDir} 0700 opengym opengym - -"
    ];

    systemd.services.opengym-api = {
      description = "openGym API";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];

      # Without this the service will happily start before the data directory's
      # filesystem is mounted, find no db.json, and come up as a blank instance
      # that people can register into — then the real data reappears underneath
      # when the dataset mounts. Refusing to start is the only safe behaviour.
      unitConfig.RequiresMountsFor = cfg.dataDir;

      environment =
        {
          # Bind to loopback: the only way in is the nginx vhost below. Upstream
          # defaults to every interface because it expects to be in a container.
          HOST = "127.0.0.1";
          PORT = toString cfg.port;
          DATA_DIR = cfg.dataDir;

          # WebAuthn relying party. RP_ID is the bare hostname and ORIGIN the
          # full URL; a mismatch does not error, it just makes every passkey fail
          # to verify. ORIGIN being https is also what flips the session cookie
          # to Secure.
          RP_ID = cfg.host;
          ORIGIN = "${scheme}://${cfg.host}";
          RP_NAME = "openGym";

          INVITE_ONLY = boolEnv cfg.inviteOnly;
          ALLOW_GUEST = boolEnv cfg.allowGuest;

          # nginx overwrites X-Forwarded-For and drops CF-Connecting-IP below, so
          # the addresses reaching the audit log are the real peer. Network only:
          # enough to tell one source from another, not where a person is.
          AUDIT_IP = "net";
        }
        // optionalAttrs cfg.oidc.enable {
          OIDC_ISSUER = cfg.oidc.issuer;
          OIDC_CLIENT_ID = cfg.oidc.clientId;
          OIDC_NAME = cfg.oidc.name;
          OIDC_SCOPES = lib.concatStringsSep " " cfg.oidc.scopes;
          OIDC_ADMIN_GROUPS = lib.concatStringsSep "," cfg.oidc.adminGroups;
          OIDC_AUTO_PROVISION = boolEnv cfg.oidc.autoProvision;
          OIDC_LINK_EXISTING = boolEnv cfg.oidc.linkExisting;
        }
        // cfg.extraEnvironment;

      serviceConfig =
        {
          ExecStart = lib.getExe cfg.package;
          User = "opengym";
          Group = "opengym";
          Restart = "on-failure";
          RestartSec = 5;
          UMask = "0077";

          ################################################################
          # Hardening. Asserted in the VM test, which fails the build if the
          # exposure score regresses.
          #
          # This is a single-file Node process that reads and writes JSON in
          # one directory, listens on one loopback port, and makes outbound
          # HTTPS to the identity provider and to browser push services. It
          # needs nothing else, so nothing else is left reachable.
          ################################################################

          # Everything read-only, /home and /root gone, and the data directory's
          # parent replaced by an empty tmpfs so that whatever else lives beside
          # it is not merely read-only but absent from this process's view.
          # BindPaths then punches back through for the one directory it owns.
          ProtectSystem = "strict";
          ProtectHome = true;
          TemporaryFileSystem = [(builtins.dirOf cfg.dataDir)];
          BindPaths = [cfg.dataDir];
          ReadWritePaths = [cfg.dataDir];
          PrivateTmp = true;

          NoNewPrivileges = true;
          CapabilityBoundingSet = [""];
          RestrictSUIDSGID = true;
          PrivateUsers = true;

          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectKernelLogs = true;
          ProtectClock = true;
          ProtectHostname = true;
          ProtectControlGroups = true;
          ProtectProc = "invisible";
          ProcSubset = "pid";
          RestrictNamespaces = true;
          LockPersonality = true;
          RestrictRealtime = true;
          RemoveIPC = true;
          KeyringMode = "private";
          PrivateDevices = true;
          DevicePolicy = "closed";

          # AF_UNIX is not optional despite there being no unix socket in the
          # app: glibc resolves hostnames by talking to nscd over one, so
          # dropping it breaks DNS and with it both SSO and push. SocketBind*
          # limits what may be listened on to the single port served; outbound
          # connections are unaffected.
          #
          # There is deliberately no IPAddressDeny/Allow pair. Egress has to
          # reach the identity provider and, for push, whatever addresses the
          # browser push services resolve to today — a set that changes under
          # us. An allow-list built from those would fail closed at the worst
          # possible moment, and an IPAddressAllow with no matching Deny is a
          # no-op that only looks like a control.
          RestrictAddressFamilies = ["AF_INET" "AF_INET6" "AF_UNIX"];
          SocketBindAllow = ["tcp:${toString cfg.port}"];
          SocketBindDeny = ["any"];

          # MemoryDenyWriteExecute is deliberately absent and must stay absent:
          # V8 JITs, so a W^X policy kills node at startup.
          SystemCallArchitectures = "native";
          SystemCallFilter = ["@system-service" "~@privileged" "~@resources" "~@obsolete"];
          SystemCallErrorNumber = "EPERM";

          # Blast radius if it is ever made to misbehave. The real process sits
          # around 80 MB with a handful of threads.
          MemoryMax = "512M";
          TasksMax = 64;
          LimitNOFILE = 4096;
        }
        // optionalAttrs (cfg.environmentFile != null) {
          EnvironmentFile = cfg.environmentFile;
        };
    };

    services.nginx = {
      enable = true;
      virtualHosts."${cfg.host}" = {
        forceSSL = cfg.useACME;
        enableACME = cfg.useACME;
        acmeRoot =
          if cfg.useACME
          then null
          else "/var/lib/acme/acme-challenge";
        http2 = true;
        root = "${cfg.webPackage}";

        locations = {
          # ^~ so that the regex location further down can never win over this
          # one. A regex match beats the longest prefix match in nginx unless
          # the prefix is marked ^~, which would silently turn any future /api/…
          # path ending in .json into a static file lookup instead of a proxied
          # request.
          "^~ /api/" = {
            proxyPass = "http://127.0.0.1:${toString cfg.port}";

            # Deliberately off. The recommended snippet is appended AFTER
            # extraConfig and sets X-Forwarded-For to $proxy_add_x_forwarded_for,
            # which prepends whatever the client sent — and openGym reads the
            # FIRST entry of that header as the caller's address for its audit
            # log. Left on, anyone could write any address they liked into the
            # log just by sending the header themselves.
            recommendedProxySettings = false;

            extraConfig = ''
              proxy_http_version 1.1;

              proxy_set_header Host $host;
              proxy_set_header X-Real-IP $remote_addr;
              proxy_set_header X-Forwarded-For $remote_addr;
              proxy_set_header X-Forwarded-Proto $scheme;
              # Never pass a client-supplied Cloudflare header through: it is
              # first in the API's list of places to look for the caller, ahead
              # of the two above, so leaving it alone would undo them.
              proxy_set_header CF-Connecting-IP "";

              # The app syncs whole-state JSON documents; the API caps a body at
              # 5 MB itself and answers 413 above that, so match rather than
              # undercut it.
              client_max_body_size 5M;
            '';
          };

          # Exercise stills and animations, straight out of the nix store. The
          # filenames carry content hashes and the store path is immutable, so
          # these can be cached as hard as the browser will allow.
          "/img/" = {
            alias = "${cfg.mediaPackage}/images/";
            extraConfig = ''
              access_log off;
              expires 30d;
              add_header Cache-Control "public, max-age=2592000, immutable";
            '';
          };
          "/gif/" = {
            alias = "${cfg.mediaPackage}/videos/";
            extraConfig = ''
              access_log off;
              expires 30d;
              add_header Cache-Control "public, max-age=2592000, immutable";
            '';
          };

          "/" = {
            tryFiles = "$uri $uri/ /index.html";
          };

          # The app shell is revalidated every load so a rebuild shows up
          # immediately rather than after a cache expiry nobody can clear
          # remotely. Media is on its own prefixes with other extensions, so
          # this cannot shadow it.
          "~* \\.(js|css|json|html)$" = {
            extraConfig = ''
              add_header Cache-Control "no-cache, must-revalidate";
            '';
          };
        };

        extraConfig = ''
          gzip on;
          gzip_types text/plain text/css application/json application/javascript image/svg+xml;
          gzip_vary on;
          gzip_min_length 1024;
        '';
      };
    };
  };
}
