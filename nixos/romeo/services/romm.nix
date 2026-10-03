{ config, pkgs, ... }:
let
  dataDirs = config.data.dirs;

  # RomM 5.3 stopped auto-detecting the library layout and refuses to start
  # ("Detected a '{platform}/roms' library layout, which is no longer
  # auto-detected") until config.yml declares it. The on-disk config.yml had
  # always been empty, so it is now owned here and mounted read-only; settings
  # changed in RomM's UI will not persist -- add them to this file instead.
  rommConfig = (pkgs.formats.yaml { }).generate "romm-config.yml" {
    filesystem.structure = {
      default = "{platform}/roms/{game}";
      firmware = "{platform}/bios";
    };
  };
in
{

  age.secrets.romm-db-password = {
    rekeyFile = ./secrets/romm_db_password.age;
    generator.script = "passphrase";
  };

  age.secrets.rom-auth-secret-key = {
    rekeyFile = ./secrets/rom_auth_secret_key.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };

  age.secrets.romm-igdb-client-secret = {
    rekeyFile = ../../../secrets/store/romeo/igdb_client_secret.age;
  };

  age.secrets.steamgriddb_api_key = {
    rekeyFile = ../../../secrets/store/romeo/steamgriddb_api_key.age;
  };

  age.secrets.romm-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/romm_auth_client_secret.age;
    generator.script = "alnum";
  };

  age-template.files.romm-env = {
    vars = {
      DB_PASSWORD = config.age.secrets.romm-db-password.path;
      AUTH_SECRET_KEY = config.age.secrets.rom-auth-secret-key.path;
      IGDB_CLIENT_SECRET = config.age.secrets.romm-igdb-client-secret.path;
      STEAMGRIDDB_API_KEY = config.age.secrets.steamgriddb_api_key.path;
      OIDC_CLIENT_SECRET = config.age.secrets.romm-oauth-client-secret.path;
    };
    content = ''
      DB_PASSWD=$DB_PASSWORD
      ROMM_AUTH_SECRET_KEY=$AUTH_SECRET_KEY
      IGDB_CLIENT_SECRET=$IGDB_CLIENT_SECRET
      STEAMGRIDDB_API_KEY=$STEAMGRIDDB_API_KEY
      OIDC_CLIENT_SECRET=$OIDC_CLIENT_SECRET
    '';
  };

  virtualisation.oci-containers.containers.romm = {
    image = "docker.io/rommapp/romm:latest";
    environment = {
      "DB_HOST" = "localhost";
      "DB_NAME" = "romm";
      "DB_USER" = "romm-user";
      "IGDB_CLIENT_ID" = "3xmoinnxfnx8caexrrx4mzq8sn3eli";
      "OIDC_ENABLED" = "true";
      "OIDC_PROVIDER" = "kanidm";
      "OIDC_CLIENT_ID" = "romm";
      "OIDC_REDIRECT_URI" = "https://rom.nel.family/api/oauth/openid";
      "OIDC_SERVER_APPLICATION_URL" = "https://idm.nel.family/oauth2/openid/romm";
    };
    environmentFiles = [
      "${config.age-template.files.romm-env.path}"
    ];
    volumes = [
      "${dataDirs.level7}/romm/resources:/romm/resources"
      "${dataDirs.level7}/romm/redis-data:/redis-data"
      "${dataDirs.level5}/romm/library:/romm/library"
      "${dataDirs.level3}/romm/assets:/romm/assets"
      "${dataDirs.level5}/romm/config:/romm/config"
      "${rommConfig}:/romm/config/config.yml:ro"
      "romm-db-sock:/run/mysqld/"
    ];
    dependsOn = ["romm-db"];
    ports = [
      "127.0.0.1:8158:8080"
    ];
    # Startup (migrations + startup tasks) takes ~20s. Without a startup check
    # the first health probe fails during it, and the failed transient unit
    # makes switch-to-configuration return 4 and fail auto-update. Same fix as
    # calibre-web-automated.nix; 45x2s = 90s fits inside the 120s timeout.
    extraOptions = [
      "--health-startup-cmd=for i in $(seq 1 45); do wget -q --spider http://127.0.0.1:8080/ && exit 0; sleep 2; done; exit 1"
      "--health-startup-timeout=120s"
      "--health-startup-success=1"
      "--health-cmd=wget -q --spider http://127.0.0.1:8080/ || exit 1"
      "--health-interval=60s"
      "--health-retries=3"
    ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  age.secrets.romm-db-root-password = {
    rekeyFile = ./secrets/romm_db_root_password.age;
    generator.script = "passphrase";
  };

  age-template.files.romm-db-env = {
    vars = {
      MARIADB_ROOT_PASSWORD = config.age.secrets.romm-db-root-password.path;
      ROMM_DB_PASSWORD = config.age.secrets.romm-db-password.path;
    };
    content = ''
      MARIADB_ROOT_PASSWORD=$MARIADB_ROOT_PASSWORD
      MARIADB_PASSWORD=$ROMM_DB_PASSWORD
    '';
  };

  virtualisation.oci-containers.containers.romm-db = {
    image = "docker.io/mariadb:11.7";
    environment = {
      "MARIADB_DATABASE" = "romm";
      "MARIADB_USER" = "romm-user";
    };
    environmentFiles = [
      "${config.age-template.files.romm-db-env.path}"
    ];
    volumes = [
      "${dataDirs.level5}/romm/db:/var/lib/mysql"
      "romm-db-sock:/run/mysqld/"
    ];
  };

  services.nginx = {
    enable = true;
    virtualHosts = {
      "rom.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        http2 = true;
        locations = {
          "/" = {
            proxyPass = "http://127.0.0.1:8158";
            proxyWebsockets = true;
            extraConfig = "client_max_body_size 100M;";
          };
        };
      };
    };
  };
}
