{
  config,
  pkgs,
  ...
}:
# Linkwarden — the household's shared bookmark manager / page archiver, at
# links.nel.family behind authentik SSO.
#
# Multi-user by design: every authentik user admitted by the application's
# policy bindings (household + extended_family, see the linkwarden blueprint on
# whiskey) gets their own Linkwarden account on first sign-in, and collections
# can be shared between those accounts from the UI.
#
# Who is admin: Linkwarden has no admin role, it treats user id 1 as the server
# admin (NEXT_PUBLIC_ADMIN defaults to 1). On a fresh database that is simply
# whoever signs in first — so bcnelson has to be the first SSO login.
#
# Local (password) accounts are switched off entirely: the sign-up form is
# closed and the credentials provider is disabled, so the login page shows only
# the authentik button. The browser extension and API clients authenticate with
# per-user access tokens created in Settings -> Access Tokens, which SSO does
# not affect.
let
  dataDirs = config.data.dirs;

  host = "links.nel.family";
  # The module default (3000) is free today but is every Node app's default;
  # pinning something less collision-prone.
  port = 8097;

  storageDir = "${dataDirs.level3}/linkwarden";
  dumpDir = "${dataDirs.level2}/linkwarden/db-dumps";
in {
  ##########################################################################
  # Secrets
  #
  # Both reach Linkwarden through one EnvironmentFile, which systemd reads as
  # root before dropping to the linkwarden user, so the agenix files stay
  # root-owned. (The module's `secretFiles` option would instead `cat` them as
  # the service user, which would force owner = "linkwarden".)
  ##########################################################################

  # NextAuth JWT signing/encryption key. Rotating it logs everybody out, nothing
  # worse: sessions are JWTs, the accounts and links live in Postgres.
  age.secrets.linkwarden-nextauth-secret = {
    rekeyFile = ./secrets/linkwarden_nextauth_secret.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -base64 32";
  };

  # Shared with authentik on whiskey — same rekeyFile on both sides, so the two
  # cannot drift.
  age.secrets.linkwarden-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/linkwarden_auth_client_secret.age;
    generator.script = "alnum";
  };

  age-template.files.linkwarden-env = {
    vars = {
      nextauthSecret = config.age.secrets.linkwarden-nextauth-secret.path;
      clientSecret = config.age.secrets.linkwarden-oauth-client-secret.path;
    };
    content = ''
      NEXTAUTH_SECRET=$nextauthSecret
      AUTHENTIK_CLIENT_SECRET=$clientSecret
    '';
  };

  ##########################################################################
  # The service
  ##########################################################################
  services.linkwarden = {
    enable = true;
    host = "127.0.0.1";
    inherit port;

    # Archives (screenshots, PDFs, single-file HTML snapshots) and uploads. On
    # the vault so the level3 snapshots + borg pick them up.
    storageLocation = storageDir;

    # Postgres is local with peer auth over /run/postgresql (module default),
    # so there is no database password to manage.
    database.createLocally = true;

    # Leaves NEXT_PUBLIC_DISABLE_REGISTRATION=true: no local sign-up form.
    # SSO sign-up is a separate switch (DISABLE_NEW_SSO_USERS, left unset), so
    # new household members still get an account on their first authentik
    # login — authentik's policy bindings are the access gate.
    enableRegistration = false;

    environmentFile = config.age-template.files.linkwarden-env.path;

    environment = {
      # NextAuth builds its callback URLs from this. It must be the public
      # https URL including the /api/v1/auth base path, or authentik rejects
      # the redirect_uri; the https:// prefix also turns on __Secure- cookies.
      NEXTAUTH_URL = "https://${host}/api/v1/auth";

      # SSO only: hide the username/password form.
      NEXT_PUBLIC_CREDENTIALS_ENABLED = "false";

      NEXT_PUBLIC_AUTHENTIK_ENABLED = "true";
      AUTHENTIK_CUSTOM_NAME = "nel.family";
      # No trailing slash: next-auth appends "/.well-known/openid-configuration"
      # to this verbatim, and a double slash 404s at authentik.
      AUTHENTIK_ISSUER = "https://auth.nel.family/application/o/linkwarden";
      AUTHENTIK_CLIENT_ID = "linkwarden";
      # AUTHENTIK_CLIENT_SECRET comes from environmentFile.
    };
  };

  # Otherwise either unit can start before the vault mounts and write archives
  # into the empty directory on the root disk underneath.
  systemd.services.linkwarden.unitConfig.RequiresMountsFor = [storageDir];
  systemd.services.linkwarden-worker.unitConfig.RequiresMountsFor = [storageDir];

  # A non-default storageLocation is not created by the module.
  systemd.tmpfiles.rules = [
    "d ${storageDir}                          0750 linkwarden linkwarden - -"
    "d ${dataDirs.level2}/linkwarden          0750 postgres   postgres   - -"
    "d ${dumpDir}                             0750 postgres   postgres   - -"
  ];

  ##########################################################################
  # Database dumps
  #
  # The live cluster is under /var/lib on the root disk, which nothing backs
  # up; these dumps are what the vault snapshots/borg capture. Not
  # services.postgresqlBackup: its `location` is global and peertube.nix
  # already points it at PeerTube's backup dir.
  ##########################################################################
  systemd.services.linkwarden-db-backup = {
    description = "Dump the Linkwarden postgres DB to the vault for backup";
    after = ["postgresql.target" "linkwarden.service"];
    unitConfig.RequiresMountsFor = [dumpDir];
    path = [config.services.postgresql.package pkgs.coreutils pkgs.findutils];
    serviceConfig = {
      Type = "oneshot";
      User = "postgres";
      Group = "postgres";
      UMask = "0077";
    };
    script = ''
      set -euo pipefail
      ts="$(date +%Y%m%d-%H%M%S)"
      pg_dump -Fc -Z 9 -f "${dumpDir}/linkwarden-$ts.dump" linkwarden
      # Keep the 14 most recent; snapshots/borg hold the longer history.
      ls -1t "${dumpDir}"/linkwarden-*.dump | tail -n +15 | xargs -r rm -f
    '';
  };

  systemd.timers.linkwarden-db-backup = {
    wantedBy = ["timers.target"];
    timerConfig = {
      OnCalendar = "*-*-* 03:30:00";
      Persistent = true;
    };
  };

  ##########################################################################
  # nginx (ACME via the host's default Porkbun DNS-01 for nel.family)
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
        # File uploads (PDFs, images) as links; Linkwarden's own default cap
        # (NEXT_PUBLIC_MAX_FILE_BUFFER) is 10 MB, this just keeps nginx's 1 MB
        # default from refusing them first.
        client_max_body_size 32M;
      '';
    };
  };
}
