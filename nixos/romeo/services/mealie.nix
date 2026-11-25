{ config, libx, ... }:
let
  dataDirs = config.data.dirs;
  smtp_password = libx.getSecret ../../sensitive.nix "smtp_password";
in
{
  # OIDC client secret, shared with the authentik provider on whiskey (same
  # rekeyFile), so the two sides can never drift. Mealie refuses to enable
  # OIDC at all without it.
  age.secrets.mealie-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/mealie_auth_client_secret.age;
    generator.script = "alnum";
  };

  age-template.files.mealie-env = {
    vars.OIDC_SECRET = config.age.secrets.mealie-oauth-client-secret.path;
    content = ''
      OIDC_CLIENT_SECRET=$OIDC_SECRET
    '';
  };

  virtualisation.oci-containers.containers.mealie = {
    environmentFiles = [ config.age-template.files.mealie-env.path ];
    image = "ghcr.io/mealie-recipes/mealie:latest";
    ports = [ "127.0.0.1:9000:9000" ];
    volumes = [ "${dataDirs.level2}/mealie:/app/data" ];
    environment = {
      ALLOW_SIGNUP = "false";
      PUID = "1000";
      PGID = "1000";
      TZ = "America/Denver";
      WEB_GUNICORN = "false";
      MAX_WORKERS = "1";
      WEB_CONCURRENCY = "1";
      BASE_URL = "https://recipes.nel.family";
      SMTP_HOST = "smtp.migadu.com";
      SMTP_PORT = "465";
      SMTP_FROM_NAME = "Nelson Family Admin";
      SMTP_AUTH_STRATEGY = "SSL";
      SMTP_FROM_EMAIL = "admin@nel.family";
      SMTP_PASSWORD = smtp_password;
      OIDC_AUTH_ENABLED = "true";
      OIDC_SIGNUP_ENABLED = "true";
      # Users are matched on the email claim (Mealie's default OIDC_USER_CLAIM).
      OIDC_CONFIGURATION_URL = "https://auth.nel.family/application/o/mealie/.well-known/openid-configuration";
      OIDC_CLIENT_ID = "mealie";
      OIDC_PROVIDER_NAME = "authentik";
      # authentik signs with its RSA self-signed key.
      OIDC_SIGNING_ALGORITHM = "RS256";
      # authentik's `groups` claim carries bare group names (kanidm sent SPNs).
      OIDC_ADMIN_GROUP = "service_admins";
    };
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}
