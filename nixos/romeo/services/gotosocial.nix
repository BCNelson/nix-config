{
  config,
  pkgs,
  ...
}: let
  dataDirs = config.data.dirs;

  host = "social.nel.family";
  # account-domain makes handles read @user@nel.family while the instance is
  # served from social.nel.family. This requires the webfinger/host-meta/nodeinfo
  # redirects on the nel.family apex vhost below. Neither value can change once
  # the instance has federated.
  accountDomain = "nel.family";
in {
  # OIDC client secret is shared with the IdP on whiskey (same rekeyFile). It is
  # injected into GoToSocial as the GTS_OIDC_CLIENT_SECRET env var rather than
  # written into the world-readable settings YAML.
  age.secrets.gotosocial-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/gotosocial_auth_client_secret.age;
    generator.script = "alnum";
  };

  age-template.files.gotosocial-env = {
    vars = {
      clientSecret = config.age.secrets.gotosocial-oauth-client-secret.path;
    };
    content = ''
      GTS_OIDC_CLIENT_SECRET=$clientSecret
    '';
  };

  services.gotosocial = {
    enable = true;
    # Provisions a local postgres instance + gotosocial db/user with peer auth
    # over the unix socket (db-address = /run/postgresql, no password needed).
    setupPostgresqlDB = true;
    environmentFile = config.age-template.files.gotosocial-env.path;

    settings = {
      host = host;
      account-domain = accountDomain;
      protocol = "https";
      # Only listen on loopback; nginx terminates TLS and proxies in.
      bind-address = "127.0.0.1";
      port = 8087;
      trusted-proxies = ["127.0.0.1/32" "::1"];

      # Media + attachments live on the vault dataset so they are captured by
      # the existing borg (level3) + sanoid snapshots. See backups note below.
      storage-backend = "local";
      storage-local-base-path = "${dataDirs.level3}/gotosocial/storage";

      # Sign-up form is closed; accounts are provisioned via OIDC on first login.
      accounts-registration-open = false;
      accounts-approval-required = true;
      # Open federation (default). Switch to "allowlist" for a closed instance.
      instance-federation-mode = "blocklist";

      oidc-enabled = true;
      oidc-idp-name = "Authentik";
      # authentik per-application issuer (parallel migration off kanidm). The
      # discovery doc is at <issuer>/.well-known/openid-configuration.
      oidc-issuer = "https://auth.nel.family/application/o/gotosocial/";
      oidc-client-id = "gotosocial";
      # oidc-client-secret comes from environmentFile (GTS_OIDC_CLIENT_SECRET).
      # authentik has no dedicated "groups" scope; group membership is delivered
      # inside the default "profile" scope claim, so requesting it is enough.
      oidc-scopes = ["openid" "email" "profile"];
      # Link OIDC logins to a pre-existing account by email.
      oidc-link-existing = true;
      # Access is already gated at authentik (the application's policy bindings
      # only admit household + extended_family), so allowed-groups stays empty.
      oidc-allowed-groups = [];
      # Admins are promoted via the CLI for reliability (see header notes). To
      # drive admin from OIDC instead, set this to an authentik group name as it
      # appears in the profile "groups" claim, e.g. [ "service_admins" ].
      oidc-admin-groups = [];
    };
  };

  # Storage dir owned by the gotosocial system user the module creates.
  # ProtectSystem=full in the unit leaves /mnt writable, so this Just Works.
  systemd.tmpfiles.rules = [
    "d ${dataDirs.level3}/gotosocial          0750 gotosocial gotosocial - -"
    "d ${dataDirs.level3}/gotosocial/storage  0750 gotosocial gotosocial - -"
    "d ${dataDirs.level2}/gotosocial          0750 postgres   postgres   - -"
    "d ${dataDirs.level2}/gotosocial/db-dumps 0750 postgres   postgres   - -"
  ];

  # Logical pg_dump into the vault for portable, restore-friendly DB backups.
  # The live postgres data dir lives under /var/lib (not on the vault), so these
  # dumps are what the borg/syncoid/sanoid jobs actually capture for the DB.
  systemd.services.gotosocial-db-backup = {
    description = "Dump GoToSocial postgres DB to the vault for backup";
    after = ["postgresql.target" "gotosocial.service"];
    path = [config.services.postgresql.package pkgs.coreutils pkgs.findutils];
    serviceConfig = {
      Type = "oneshot";
      User = "postgres";
      Group = "postgres";
    };
    script = ''
      set -euo pipefail
      dir="${dataDirs.level2}/gotosocial/db-dumps"
      ts="$(date +%Y%m%d-%H%M%S)"
      # -Fc is already zlib-compressed (default level 6); -Z 9 maxes the ratio.
      # pg14 only ships zlib here; zstd/lz4 dump compression needs pg16+.
      pg_dump -Fc -Z 9 -f "$dir/gotosocial-$ts.dump" gotosocial
      # Keep the 14 most recent dumps; snapshots/borg hold the longer history.
      ls -1t "$dir"/gotosocial-*.dump | tail -n +15 | xargs -r rm -f
    '';
  };

  systemd.timers.gotosocial-db-backup = {
    wantedBy = ["timers.target"];
    timerConfig = {
      OnCalendar = "*-*-* 03:00:00";
      Persistent = true;
    };
  };

  services.nginx.virtualHosts = {
    # Public entrypoint for the instance + ActivityPub federation.
    "${host}" = {
      forceSSL = true;
      enableACME = true;
      acmeRoot = null;
      http2 = true;
      # GoToSocial's default media limits top out around 40M.
      extraConfig = "client_max_body_size 40M;";
      locations."/" = {
        proxyPass = "http://127.0.0.1:8087";
        proxyWebsockets = true;
      };
    };

    # Apex vhost: redirect the .well-known endpoints to the real host so that
    # @user@nel.family handles resolve. Required because account-domain != host.
    "${accountDomain}" = {
      forceSSL = true;
      enableACME = true;
      acmeRoot = null;
      locations = {
        "/.well-known/webfinger".extraConfig = ''
          rewrite ^.*$ https://${host}/.well-known/webfinger permanent;
        '';
        "/.well-known/host-meta".extraConfig = ''
          rewrite ^.*$ https://${host}/.well-known/host-meta permanent;
        '';
        "/.well-known/nodeinfo".extraConfig = ''
          rewrite ^.*$ https://${host}/.well-known/nodeinfo permanent;
        '';
        "/".extraConfig = ''
          return 302 https://${host};
        '';
      };
    };
  };
}
