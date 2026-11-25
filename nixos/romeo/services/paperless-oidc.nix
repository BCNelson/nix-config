# Paperless's OIDC login (django-allauth) through authentik. Paperless itself
# runs in the "general" compose stack (docker-services/defs/paperless.nix),
# which reads the env file rendered here; it lives in a module because the
# compose definitions are plain functions with no access to `config`.
{ config, ... }:
{
  # Shared with the authentik provider on whiskey (same rekeyFile), so the two
  # sides can never drift.
  age.secrets.paperless-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/paperless_auth_client_secret.age;
    generator.script = "alnum";
  };

  # allauth takes its whole provider list, secret included, as one JSON env
  # var. Single-quoted because compose's env_file parser would otherwise try to
  # interpret the JSON; envsubst only replaces $OIDC_SECRET.
  age-template.files.paperless-env = {
    vars.OIDC_SECRET = config.age.secrets.paperless-oauth-client-secret.path;
    content = ''
      PAPERLESS_SOCIALACCOUNT_PROVIDERS='${builtins.toJSON {
        openid_connect = {
          OAUTH_PKCE_ENABLED = true;
          APPS = [
            {
              # Part of the callback URL registered in authentik
              # (blueprints/paperless.yaml), and of every connected account's
              # link: changing it disconnects everyone.
              provider_id = "authentik";
              name = "authentik";
              client_id = "paperless";
              secret = "$OIDC_SECRET";
              settings.server_url = "https://auth.nel.family/application/o/paperless/.well-known/openid-configuration";
            }
          ];
        };
      }}'
    '';
  };
}
