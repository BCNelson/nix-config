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
    extraOptions = [ "--pod=romm" ];
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
    extraOptions = [ "--pod=romm" ];
  };

  systemd.services.create-romm-pod = with config.virtualisation.oci-containers; {
    serviceConfig.Type = "oneshot";
    wantedBy = [ "${backend}-romm-db.service" "${backend}-romm.service" ];
    script = ''
      ${pkgs.podman}/bin/podman pod exists romm || \
        ${pkgs.podman}/bin/podman pod create -n romm -p '0.0.0.0:8091:8080'
    '';
  };

  services.nginx = {
    enable = true;
    virtualHosts = {
      "rom.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        locations = {
          "/" = {
            proxyPass = "http://localhost:8091";
            extraConfig = ''
              proxy_set_header Host $host;
            '';
          };
        };
      };
    };
  };
}
