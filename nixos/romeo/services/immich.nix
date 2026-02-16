{ config, libx, pkgs, ... }:
let
  dataDirs = config.data.dirs;
  immich_postgres_password = libx.getSecret ../../sensitive.nix "immich_postgres_password";
  networkEnsure = [ "-${pkgs.podman}/bin/podman network create immich" ];
  commonVolumes = [
    "${dataDirs.level2}/immich/photos:/usr/src/app/upload"
    "${dataDirs.level6}/immich/encoded-video:/usr/src/app/upload/encoded-video"
    "${dataDirs.level6}/immich/thumbs:/usr/src/app/upload/thumbs"
    "/etc/localtime:/etc/localtime:ro"
  ];
in
{
  virtualisation.oci-containers.containers.immich-server = {
    image = "ghcr.io/immich-app/immich-server:release";
    volumes = commonVolumes;
    ports = [ "127.0.0.1:2283:2283" ];
    environment = {
      "DB_HOSTNAME" = "immich-postgres";
      "DB_USERNAME" = "postgres";
      "DB_DATABASE_NAME" = "immich";
      "DB_PASSWORD" = immich_postgres_password;
      "REDIS_HOSTNAME" = "immich-redis";
    };
    dependsOn = [ "immich-redis" "immich-postgres" ];
    extraOptions = [ "--network=immich" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  virtualisation.oci-containers.containers.immich-machine-learning = {
    image = "ghcr.io/immich-app/immich-machine-learning:release";
    volumes = [
      "${dataDirs.level7}/immich:/cache"
    ];
    dependsOn = [ "immich-redis" ];
    extraOptions = [ "--network=immich" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  virtualisation.oci-containers.containers.immich-redis = {
    image = "redis:6.2-alpine";
    extraOptions = [ "--network=immich" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  virtualisation.oci-containers.containers.immich-postgres = {
    image = "tensorchord/pgvecto-rs:pg14-v0.2.0";
    environment = {
      "POSTGRES_PASSWORD" = immich_postgres_password;
      "POSTGRES_USER" = "postgres";
      "POSTGRES_DB" = "immich";
    };
    volumes = [
      "${dataDirs.level2}/immich/database:/var/lib/postgresql/data"
    ];
    dependsOn = [ "immich-redis" ];
    extraOptions = [ "--network=immich" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  # Ensure the immich network exists before the first container starts
  # Redis starts first (others dependsOn it), so only it needs this
  systemd.services.podman-immich-redis.serviceConfig.ExecStartPre = networkEnsure;
}
