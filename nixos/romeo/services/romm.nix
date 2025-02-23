{ config, pkgs, ... }:
let
  dataDirs = config.data.dirs;
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

  age-template.files.romm-env = {
    vars = {
      DB_PASSWORD = config.age.secrets.romm-db-password.path;
      AUTH_SECRET_KEY = config.age.secrets.rom-auth-secret-key.path;
      IGDB_CLIENT_SECRET = config.age.secrets.romm-igdb-client-secret.path;
      STEAMGRIDDB_API_KEY = config.age.secrets.steamgriddb_api_key.path;
    };
    content = ''
      DB_PASSWD=$DB_PASSWORD
      ROMM_AUTH_SECRET_KEY=$AUTH_SECRET_KEY
      IGDB_CLIENT_SECRET=$IGDB_CLIENT_SECRET
      STEAMGRIDDB_API_KEY=$STEAMGRIDDB_API_KEY
    '';
  };

  virtualisation.oci-containers.containers.romm = {
    image = "rommapp/romm:latest";
    environment = {
      "DB_HOST" = "localhost";
      "DB_NAME" = "romm";
      "DB_USER" = "romm-user";
      "IGDB_CLIENT_ID" = "3xmoinnxfnx8caexrrx4mzq8sn3eli";
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
      "romm-db-sock:/run/mysqld/"
    ];
    dependsOn = ["romm-db"];
    ports = [
      "127.0.0.1:8158:8080"
    ];
    extraOptions = [
      "--health-cmd=wget -q --spider http://127.0.0.1:8080/ || exit 1"
      "--health-interval=10s"
      "--health-retries=3"
    ];
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
    image = "mariadb:latest";
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
            extraConfig = ''
              proxy_set_header Host $host;
              
              # Hide version
              server_tokens off;

              # Security headers
              add_header X-Frame-Options "SAMEORIGIN" always;
              add_header X-Content-Type-Options "nosniff" always;
              add_header X-XSS-Protection "1; mode=block" always;
              add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
              add_header Referrer-Policy "no-referrer-when-downgrade" always;
            '';
          };
        };
      };
    };
  };
}
