{ dataDirs, libx }:
let
  immich_postgres_password = libx.getSecret ../../../sensitive.nix "immich_postgres_password";
  commonVolumes = [
    "${dataDirs.level2}/immich/photos:/usr/src/app/upload"
    "${dataDirs.level6}/immich/encoded-video:/usr/src/app/upload/encoded-video"
    "${dataDirs.level6}/immich/thumbs:/usr/src/app/upload/thumbs"
    "/etc/localtime:/etc/localtime:ro"
  ];
in
{
  immich-server = {
    image = "ghcr.io/immich-app/immich-server:release";
    container_name = "immich_server";
    volumes = commonVolumes;
    ports = [ "127.0.0.1:2283:2283" ];
    environment = [
      "DB_HOSTNAME=immich_postgres"
      "DB_USERNAME=postgres"
      "DB_DATABASE_NAME=immich"
      "DB_PASSWORD=${immich_postgres_password}"
      "REDIS_HOSTNAME=immich_redis"
    ];
    depends_on = [ "immich_redis" "immich_postgres" ];
    restart = "unless-stopped";
  };

  immich-machine-learning = {
    image = "ghcr.io/immich-app/immich-machine-learning:release";
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
    image = "tensorchord/pgvecto-rs:pg14-v0.2.0";
    container_name = "immich_postgres";
    environment = [
      "POSTGRES_PASSWORD=${immich_postgres_password}"
      "POSTGRES_USER=postgres"
      "POSTGRES_DB=immich"
    ];
    volumes = [
      "${dataDirs.level2}/immich/database:/var/lib/postgresql/data"
    ];
    restart = "unless-stopped";
  };
}
