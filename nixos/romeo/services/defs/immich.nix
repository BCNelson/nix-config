{ dataDirs }:
let
  sensitiveData = import ../../../sensitive.nix;
in
{
  immich-server = {
    image = "ghcr.io/immich-app/immich-server:latest";
    container_name = "immich_server";
    command = [ "start.sh" "immich" ];
    volumes = [
      "${dataDirs.level2}/immich/photos:/usr/src/app/upload"
      "/etc/localtime:/etc/localtime:ro"
    ];
    environment = [
      "DB_HOSTNAME=immich_postgres"
      "DB_USERNAME=postgres"
      "DB_DATABASE_NAME=immich"
      "DB_PASSWORD=${sensitiveData.immich_postgres_password}"
      "REDIS_HOSTNAME=immich_redis"
    ];
    depends_on = [ "immich_redis" "immich_postgres" ];
    restart = "unless-stopped";
  };

  immich-microservices = {
    image = "ghcr.io/immich-app/immich-server:latest";
    container_name = "immich_microservices";
    command = [ "start.sh" "microservices" ];
    volumes = [
      "${dataDirs.level5}/immich/photos:/usr/src/app/upload"
      "/etc/localtime:/etc/localtime:ro"
    ];
    environment = [
      "DB_HOSTNAME=immich_postgres"
      "DB_USERNAME=postgres"
      "DB_DATABASE_NAME=immich"
      "DB_PASSWORD=${sensitiveData.immich_postgres_password}"
      "REDIS_HOSTNAME=immich_redis"
    ];
    devices = [ "/dev/dri:/dev/dri" ];
    depends_on = [ "immich_redis" "immich_postgres" ];
    restart = "unless-stopped";
  };

  immich-machine-learning = {
    image = "ghcr.io/immich-app/immich-machine-learning:latest";
    container_name = "immich_machine_learning";
    volumes = [
      "${dataDirs.level7}/immich:/cache"
    ];
    environment = [
    ];
    restart = "unless-stopped";
  };

  immich_redis = {
    image = "redis:6.2-alpine";
    restart = "unless-stopped";
    container_name = "immich_redis";
  };

  immich_postgres = {
    image = "tensorchord/pgvecto-rs:pg14-v0.1.11";
    container_name = "immich_postgres";
    environment = [
      "POSTGRES_PASSWORD=${sensitiveData.immich_postgres_password}"
      "POSTGRES_USER=postgres"
      "POSTGRES_DB=immich"
    ];
    volumes = [
      "${dataDirs.level2}/immich/database:/var/lib/postgresql/data"
    ];
    restart = "unless-stopped";
  };
}
