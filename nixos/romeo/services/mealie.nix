{ config, libx, ... }:
let
  dataDirs = config.data.dirs;
  smtp_password = libx.getSecret ../../sensitive.nix "smtp_password";
in
{
  virtualisation.oci-containers.containers.mealie = {
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
      OIDC_CONFIGURATION_URL = "https://idm.nel.family/oauth2/openid/mealie/.well-known/openid-configuration";
      OIDC_CLIENT_ID = "mealie";
      OIDC_PROVIDER_NAME = "Kanidm";
      OIDC_SIGNING_ALGORITHM = "ES256";
      OIDC_ADMIN_GROUP = "service_admins@nel.family";
    };
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}
