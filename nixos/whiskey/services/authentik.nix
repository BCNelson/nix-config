{
  config,
  pkgs,
  libx,
  ...
}: let
  # Shared family Migadu SMTP password (git-crypt sensitive.nix), reused by
  # mealie/vikunja/vaultwarden. authenticates as admin@nel.family on :465 SSL.
  smtp_password = libx.getSecret ../../sensitive.nix "smtp_password";
  # authentik scans a single blueprints directory recursively. The upstream
  # package ships the default blueprints (default flows, stages, etc.) that our
  # custom blueprints reference via !Find, so we merge upstream + ours into one
  # directory and point blueprints_dir at the result.
  mergedBlueprints = pkgs.symlinkJoin {
    name = "authentik-blueprints";
    paths = [
      "${config.services.authentik.authentikComponents.staticWorkdirDeps}/blueprints"
      ./authentik/blueprints
    ];
  };
in {
  ##########################################################################
  # Secrets (agenix-rekey)
  #
  # authentik runs as a DynamicUser, so there is no static "authentik" user at
  # agenix-activation time. environmentFile is consumed via systemd
  # EnvironmentFile (read by root before privilege drop), so secrets stay
  # root-owned — do NOT set owner = "authentik".
  ##########################################################################

  # Django SECRET_KEY. Generated.
  age.secrets.authentik-secret-key = {
    rekeyFile = ./secrets/authentik_secret_key.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -base64 50";
  };

  # Initial akadmin bootstrap password (first-login admin). Generated.
  age.secrets.authentik-bootstrap-password = {
    rekeyFile = ./secrets/authentik_bootstrap_password.age;
    generator.script = "passphrase";
    bitwarden = {
      name = "Authentik akadmin";
      username = "akadmin";
    };
  };

  # OAuth2 client secret for the gotosocial provider — REUSED from the existing
  # shared store so the gotosocial service's secret file is unchanged.
  age.secrets.gotosocial-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/gotosocial_auth_client_secret.age;
    generator.script = "alnum";
  };

  # Single env file assembled from the individual secrets. The names on the
  # right of `vars` are shell variables substituted into `content`.
  age-template.files.authentik-env = {
    vars = {
      SECRET_KEY = config.age.secrets.authentik-secret-key.path;
      BOOTSTRAP_PASSWORD = config.age.secrets.authentik-bootstrap-password.path;
      GOTOSOCIAL_SECRET = config.age.secrets.gotosocial-oauth-client-secret.path;
    };
    content = ''
      AUTHENTIK_SECRET_KEY=$SECRET_KEY
      AUTHENTIK_BOOTSTRAP_PASSWORD=$BOOTSTRAP_PASSWORD
      AUTHENTIK_BOOTSTRAP_EMAIL=bradley@nel.family
      GOTOSOCIAL_OAUTH_CLIENT_SECRET=$GOTOSOCIAL_SECRET
    '';
  };

  ##########################################################################
  # authentik (authentik-nix). Manages its own PostgreSQL + Redis.
  ##########################################################################
  services.authentik = {
    enable = true;
    createDatabase = true;
    environmentFile = config.age-template.files.authentik-env.path;

    # We terminate TLS at our own nginx vhost using the shared porkbun-ACME
    # defaults, so the module's bundled nginx is disabled.
    nginx.enable = false;

    settings = {
      disable_startup_analytics = true;
      avatars = "initials";
      email = {
        host = "smtp.migadu.com";
        port = 465;
        username = "admin@nel.family";
        password = smtp_password;
        use_ssl = true;
        use_tls = false;
        from = "admin@nel.family";
      };
      # Merge our custom blueprints with the upstream defaults.
      blueprints_dir = "${mergedBlueprints}";
    };
  };

  ##########################################################################
  # nginx vhost (reuses security.acme.defaults from roles/server/nginx.nix).
  # authentik's server listens on https://localhost:9443 (self-signed; nginx
  # does not verify upstream certs by default).
  ##########################################################################
  services.nginx.virtualHosts."auth.nel.family" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 512M;
    '';
    locations."/" = {
      proxyPass = "https://localhost:9443";
      proxyWebsockets = true;
    };
  };
}
