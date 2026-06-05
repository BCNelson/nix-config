{ config, pkgs, ... }:
let
  dataDirs = config.data.dirs;

  appImage = "docker.io/swalabtech/journiv-app:latest";
  appNetwork = "journiv";
  appVolumes = [ "${dataDirs.level3}/journiv/data:/data" ];
  appExtraOptions = [ "--network=${appNetwork}" ];
  appEnvFiles = [ "${config.age-template.files.journiv-env.path}" ];

  fullAppEnv = {
    "DB_DRIVER" = "postgres";
    "POSTGRES_HOST" = "journiv-postgres";
    "POSTGRES_USER" = "journiv";
    "POSTGRES_DB" = "journiv_prod";
    "POSTGRES_PORT" = "5432";
    "REDIS_URL" = "redis://journiv-valkey:6379/0";
    "CELERY_BROKER_URL" = "redis://journiv-valkey:6379/0";
    "CELERY_RESULT_BACKEND" = "redis://journiv-valkey:6379/0";
    "DOMAIN_NAME" = "journal.nel.family";
    "DOMAIN_SCHEME" = "https";
    "OIDC_ENABLED" = "true";
    "OIDC_ISSUER" = "https://idm.nel.family/oauth2/openid/journiv";
    "OIDC_CLIENT_ID" = "journiv";
    "OIDC_REDIRECT_URI" = "https://journal.nel.family/api/v1/auth/oidc/callback";
    "OIDC_AUTO_PROVISION" = "true";
    "OIDC_SCOPES" = "openid email profile";
  };
in
{
  age.secrets.journiv-secret-key = {
    rekeyFile = ./secrets/journiv_secret_key.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -base64 32";
  };

  age.secrets.journiv-postgres-password = {
    rekeyFile = ./secrets/journiv_postgres_password.age;
    generator.script = "passphrase";
  };

  age.secrets.journiv-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/journiv_auth_client_secret.age;
    generator.script = "alnum";
  };

  age-template.files.journiv-env = {
    vars = {
      SECRET_KEY = config.age.secrets.journiv-secret-key.path;
      POSTGRES_PASSWORD = config.age.secrets.journiv-postgres-password.path;
      OIDC_CLIENT_SECRET = config.age.secrets.journiv-oauth-client-secret.path;
    };
    content = ''
      SECRET_KEY=$SECRET_KEY
      POSTGRES_PASSWORD=$POSTGRES_PASSWORD
      OIDC_CLIENT_SECRET=$OIDC_CLIENT_SECRET
    '';
  };

  age-template.files.journiv-postgres-env = {
    vars = {
      POSTGRES_PASSWORD = config.age.secrets.journiv-postgres-password.path;
    };
    content = ''
      POSTGRES_PASSWORD=$POSTGRES_PASSWORD
    '';
  };

  virtualisation.oci-containers.containers.journiv-valkey = {
    image = "docker.io/valkey/valkey:9.0-alpine";
    volumes = [
      "${dataDirs.level7}/journiv/valkey:/data"
    ];
    extraOptions = appExtraOptions ++ [
      "--health-cmd=valkey-cli ping || exit 1"
      "--health-interval=10s"
      "--health-retries=5"
    ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  virtualisation.oci-containers.containers.journiv-postgres = {
    image = "docker.io/postgres:18.1";
    environment = {
      "POSTGRES_USER" = "journiv";
      "POSTGRES_DB" = "journiv_prod";
    };
    environmentFiles = [
      "${config.age-template.files.journiv-postgres-env.path}"
    ];
    volumes = [
      "${dataDirs.level2}/journiv/postgres:/var/lib/postgresql"
    ];
    dependsOn = [ "journiv-valkey" ];
    extraOptions = appExtraOptions ++ [
      "--health-cmd=pg_isready -U journiv -d journiv_prod"
      "--health-interval=10s"
      "--health-retries=5"
    ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  virtualisation.oci-containers.containers.journiv-app = {
    image = appImage;
    environment = fullAppEnv // {
      "SERVICE_ROLE" = "app";
      "ENVIRONMENT" = "production";
      "RATE_LIMIT_STORAGE_URI" = "redis://journiv-valkey:6379/1";
    };
    environmentFiles = appEnvFiles;
    volumes = appVolumes;
    ports = [ "127.0.0.1:8090:8000" ];
    dependsOn = [ "journiv-postgres" "journiv-valkey" ];
    extraOptions = appExtraOptions;
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  virtualisation.oci-containers.containers.journiv-worker = {
    image = appImage;
    cmd = [
      "celery" "-A" "app.core.celery_app" "worker"
      "--loglevel=info"
      "--concurrency=1"
      "--max-memory-per-child=300000"
      "--max-tasks-per-child=200"
    ];
    environment = fullAppEnv // {
      "SERVICE_ROLE" = "celery-worker";
    };
    environmentFiles = appEnvFiles;
    volumes = appVolumes;
    dependsOn = [ "journiv-app" ];
    extraOptions = appExtraOptions;
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  virtualisation.oci-containers.containers.journiv-beat = {
    image = appImage;
    cmd = [
      "celery" "-A" "app.core.celery_app" "beat"
      "--loglevel=info"
      "--scheduler" "redbeat.RedBeatScheduler"
      "--pidfile=/tmp/celerybeat.pid"
    ];
    environment = fullAppEnv // {
      "SERVICE_ROLE" = "celery-beat";
      "REDBEAT_REDIS_URL" = "redis://journiv-valkey:6379/2";
    };
    environmentFiles = appEnvFiles;
    volumes = appVolumes;
    dependsOn = [ "journiv-app" ];
    extraOptions = appExtraOptions;
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  systemd.tmpfiles.rules = [
    "d ${dataDirs.level2}/journiv          0755 root root - -"
    "d ${dataDirs.level2}/journiv/postgres 0755 root root - -"
    "d ${dataDirs.level3}/journiv          0755 root root - -"
    "d ${dataDirs.level3}/journiv/data     0755 1000 1000 - -"
    "d ${dataDirs.level7}/journiv          0755 root root - -"
    "d ${dataDirs.level7}/journiv/valkey   0755 root root - -"
  ];

  systemd.services.podman-journiv-valkey.serviceConfig.ExecStartPre = [
    "-${pkgs.podman}/bin/podman network create ${appNetwork}"
  ];

  services.nginx = {
    enable = true;
    virtualHosts = {
      "journal.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        http2 = true;
        locations = {
          "/" = {
            proxyPass = "http://127.0.0.1:8090";
            proxyWebsockets = true;
            extraConfig = "client_max_body_size 500M;";
          };
        };
      };
    };
  };
}
