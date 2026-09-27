{config, ...}:
# Trilium Notes sync server — the hub the trilium-desktop instances on the
# workstations sync against (home-manager/bcnelson/_mixins/programs/trillium.nix).
#
# Two independent authentication paths, which is the thing to keep straight:
#
#   browser   OIDC against authentik. Once mfaMethod=oauth is set in Trilium's
#             DB, the password form on /login is REPLACED by a "sign in with
#             Authentik" button -- there is no password fallback left in the UI.
#   desktop   the document secret. Each sync request is an HMAC of
#             `documentSecret` (POST /api/login/sync), which OIDC does not touch.
#             Only the one-time bootstrap of a new client reads the password
#             (GET /api/setup/sync-seed -> checkCredentials), and that route
#             verifies the password alone and never consults mfaMethod.
#
# So enabling OIDC cannot lock a desktop out of sync, and the local password has
# to stay set regardless -- Trilium refuses to enable OIDC at all until it is
# (isPasswordSet gates isOpenIDConfigured).
#
# Turning OIDC on is a two-step process and the second step is manual, by
# design: the values below only make the provider *available*. Linking it to the
# account is Options -> Password & Auth -> "Sign-in with: OpenID Connect
# provider" -> "Connect account", which writes the authentik subject into the DB.
# Until that link exists Trilium keeps using the password, so a broken OIDC
# config cannot lock anyone out.
let
  dataDirs = config.data.dirs;

  host = "notes.bcnelson.dev";
  port = 8319;
  dataDir = "${dataDirs.level3}/trilium";
in {
  ##########################################################################
  # Secrets
  #
  # Shared with authentik on whiskey — same rekeyFile on both sides, so the two
  # cannot drift. Reaches the service as an EnvironmentFile, which systemd reads
  # as root before dropping privileges, so the file stays root-owned.
  ##########################################################################
  age.secrets.trilium-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/trilium_auth_client_secret.age;
    generator.script = "alnum";
  };

  age-template.files.trilium-env = {
    vars.clientSecret = config.age.secrets.trilium-oauth-client-secret.path;
    content = ''
      TRILIUM_MULTIFACTORAUTHENTICATION_OAUTHCLIENTSECRET=$clientSecret
    '';
  };

  ##########################################################################
  # The server
  ##########################################################################
  services.trilium-server = {
    enable = true;
    instanceName = "romeo";
    inherit port dataDir;
    host = "127.0.0.1";

    # Trilium's own periodic backups (dataDir/backup) are what makes the level3
    # snapshot + borg jobs usable: borg copies a live SQLite file and can catch
    # it mid-write, while the backup/ copies are consistent by construction.
    noBackup = false;

    # nginx is configured by hand below: the module's vhost has no ACME, no
    # proxy buffer sizing, and would not know about the Cloudflare DNS-01
    # override this zone needs.
    nginx.enable = false;

    environmentFile = config.age-template.files.trilium-env.path;
  };

  # OIDC and reverse-proxy settings, which the module has no options for.
  # Trilium reads TRILIUM_<SECTION>_<KEY> independently of config.ini, so these
  # work even though the module's generated config.ini has no
  # [MultiFactorAuthentication] section at all.
  systemd.services.trilium-server.environment = {
    TRILIUM_MULTIFACTORAUTHENTICATION_OAUTHBASEURL = "https://${host}";
    TRILIUM_MULTIFACTORAUTHENTICATION_OAUTHCLIENTID = "trilium";
    # The discovery URL rather than the bare issuer: Trilium uses any URL
    # containing /.well-known/ as-is, which stays correct whether authentik is
    # in per-provider or global issuer mode. A bare issuer is matched against
    # what the provider advertises, so a later issuer_mode change would break it.
    TRILIUM_MULTIFACTORAUTHENTICATION_OAUTHISSUERBASEURL = "https://auth.nel.family/application/o/trilium/.well-known/openid-configuration";
    TRILIUM_MULTIFACTORAUTHENTICATION_OAUTHISSUERNAME = "Authentik";

    # express `trust proxy`. Without it Trilium sees every request as coming
    # from 127.0.0.1 over http, which puts the wrong address in the rate-limit
    # buckets and the "wrong password from <ip>" log lines.
    TRILIUM_NETWORK_TRUSTEDREVERSEPROXY = "loopback";
  };

  systemd.services.trilium-server.serviceConfig = {
    # ProtectSystem=strict makes / read-only, and the database lives outside the
    # usual StateDirectory tree, so it has to be named explicitly.
    ReadWritePaths = [dataDir];

    Restart = "on-failure";
    RestartSec = 5;
    UMask = "0077";

    ProtectSystem = "strict";
    ProtectHome = true;
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
    # Outbound HTTPS is needed to reach authentik's discovery and token
    # endpoints, and for link-preview fetches.
    RestrictAddressFamilies = ["AF_INET" "AF_INET6" "AF_UNIX"];
    # No MemoryDenyWriteExecute: V8 JITs.
    SystemCallArchitectures = "native";
    SystemCallFilter = ["@system-service" "~@privileged" "~@resources"];
  };

  # Otherwise it can start before the vault mounts and initialise a fresh, empty
  # document on the underlying directory — which for a sync hub means the next
  # desktop to connect either refuses or reseeds itself from nothing.
  systemd.services.trilium-server.unitConfig.RequiresMountsFor = [dataDir];

  ##########################################################################
  # TLS
  #
  # bcnelson.dev is a Cloudflare zone, so this cert overrides the host default
  # (Porkbun) with a Cloudflare DNS-01 challenge. Same token file whiskey uses
  # for git.bcnelson.dev, rekeyed for romeo. Pattern mirrors the *.cwnel.com
  # cert in nixos/romeo/default.nix; dnsResolver comes from the host-wide
  # security.acme.defaults there, which every DNS-01 on romeo needs in order to
  # bypass the local unbound.
  ##########################################################################
  age.secrets.cloudflare_dns_api_token.rekeyFile = ../../../secrets/store/cloudflare_dns_api_token.age;

  age-template.files."bcnelson-dev-cloudflare-acme-env" = {
    vars.token = config.age.secrets.cloudflare_dns_api_token.path;
    content = "CF_DNS_API_TOKEN=$token";
  };

  security.acme.certs.${host} = {
    dnsProvider = "cloudflare";
    environmentFile = config.age-template.files."bcnelson-dev-cloudflare-acme-env".path;
  };

  ##########################################################################
  # nginx
  ##########################################################################
  services.nginx.virtualHosts.${host} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    http2 = true;
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString port}";
      proxyWebsockets = true;
      extraConfig = ''
        # On /callback Trilium's response carries the freshly minted session
        # alongside the OIDC state cookies, which overruns nginx's default 4k
        # upstream-header buffer. nginx drops the response and the sign-in
        # surfaces as "invalid user", not as a header error — upstream documents
        # these exact sizes as the fix.
        proxy_buffer_size 128k;
        proxy_buffers 4 256k;
        proxy_busy_buffers_size 256k;

        # Note attachments have no fixed ceiling, and a sync seed carries the
        # whole document.
        client_max_body_size 0;

        # Restoring a large document over sync can sit well past the 60s default.
        proxy_read_timeout 300s;
        proxy_send_timeout 300s;
      '';
    };
  };
}
