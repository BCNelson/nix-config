{ dataDirs, libx }:
let
  SECRET_KEY = libx.getSecret ../../../sensitive.nix "paperless_secret_key";
  paperAuthConfig = builtins.toJSON {
    openid_connect = {
      OAUTH_PKCE_ENABLED = true;
      APPS = [
        {
          provider_id = "kanidm";
          name = "Kanidm";
          client_id = "paperless";
          secret = "${libx.getSecret ../../../sensitive.nix "kanidm_paperless_client_secret"}";
          settings = {
            server_url = "https://idm.nel.family/oauth2/openid/paperless/.well-known/openid-configuration";
          };
        }
      ];
    };
  };
in
{
  broker = {
    image = "docker.io/library/redis:7";
    restart = "unless-stopped";
  };

  paperless = {
    image = "ghcr.io/paperless-ngx/paperless-ngx:latest";
    restart = "unless-stopped";
    depends_on = [ "broker" "gotenberg" "tika" ];
    healthcheck = {
      test = [ "CMD" "curl" "-f" "http://localhost:8000" ];
      interval = "30s";
      timeout = "10s";
      retries = 5;
    };
    volumes = [
      "${dataDirs.level1}/paperless/data:/usr/src/paperless/data"
      "${dataDirs.level1}/paperless/media:/usr/src/paperless/media"
      "${dataDirs.level3}/paperless/export:/usr/src/paperless/export"
      "${dataDirs.level3}/paperless/consume:/usr/src/paperless/consume"
    ];
    ports = [ "127.0.0.1:8000:8000" ];
    environment = {
      PAPERLESS_REDIS = "redis://broker:6379";
      PAPERLESS_TIKA_ENABLED = "1";
      PAPERLESS_TIKA_GOTENBERG_ENDPOINT = "http://gotenberg:3000";
      PAPERLESS_TIKA_ENDPOINT = "http://tika:9998";
      PAPERLESS_SECRET_KEY = SECRET_KEY;
      PAPERLESS_TIME_ZONE = "America/Denver";
      PAPERLESS_OCR_LANGUAGE = "eng";
      PAPERLESS_DEBUG = "True";
      PAPERLESS_URL = "https://docs.h.b.nel.family";
      PAPERLESS_CONSUMER_RECURSIVE = "true";
      PAPERLESS_CONSUMPTION_DIR = "/usr/src/paperless/consume";
      PAPERLESS_DATA_DIR = "/usr/src/paperless/data";
      PAPERLESS_MEDIA_ROOT = "/usr/src/paperless/media";
      PAPERLESS_APPS = "allauth.socialaccount.providers.openid_connect";
      PAPERLESS_SOCIALACCOUNT_PROVIDERS = "${paperAuthConfig}";
    };
  };

  gotenberg = {
    image = "docker.io/gotenberg/gotenberg:7.8";
    restart = "unless-stopped";
    environment = {
      DISABLE_GOOGLE_CHROME = "1";
    };
  };

  tika = {
    image = "ghcr.io/paperless-ngx/tika:latest";
    restart = "unless-stopped";
  };
}
