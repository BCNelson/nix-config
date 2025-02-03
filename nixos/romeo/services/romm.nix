{ config, ... }:
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

  age-template.files.romm-env = {
    vars = {
      DB_PASSWORD = config.age.secrets.romm-db-password.path;
      AUTH_SECRET_KEY = config.age.secrets.rom-auth-secret-key.path;
    };
    content = ''
      DB_PASSWD=$DB_PASSWORD
      ROMM_AUTH_SECRET_KEY=$AUTH_SECRET_KEY
    '';
  };

  virtualisation.oci-containers.containers.romm = {
    image = "rommapp/romm:latest";
    environment = {
      "DB_HOST" = "romm-db";
      "DB_NAME" = "romm";
      "DB_USER" = "romm-user";
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
    ];
    ports = [ "127.0.0.1:8090:80" ];
    dependsOn = ["romm-db"];
    networks = ["romm"];
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
    ];
    networks = ["romm"];
  };

  services.nginx = {
    enable = true;
    virtualHosts = {
      "rom.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8090";
            extraConfig = ''
              proxy_set_header Host $host;
              proxy_set_header Upgrade $http_upgrade;
              proxy_set_header Connection "upgrade";
            '';
          };
        };
      };
    };
  };
}
