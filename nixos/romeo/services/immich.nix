{ config, libx, pkgs, ... }:
let
  dataDirs = config.data.dirs;
  immich_postgres_password = libx.getSecret ../sensitive.nix "immich_postgres_password";
  commonVolumes = [
    "${dataDirs.level2}/immich/photos:/usr/src/app/upload"
    "${dataDirs.level6}/immich/encoded-video:/usr/src/app/upload/encoded-video"
    "${dataDirs.level6}/immich/thumbs:/usr/src/app/upload/thumbs"
    "/etc/localtime:/etc/localtime:ro"
  ];
in
{
  systemd.services.docker-network-immich = {
    description = "Create Docker network for Immich";
    after = [ "docker.service" ];
    requires = [ "docker.service" ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };
    script = "${pkgs.docker}/bin/docker network inspect immich >/dev/null 2>&1 || ${pkgs.docker}/bin/docker network create immich";
    wantedBy = [ "multi-user.target" ];
  };

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
    extraOptions = [ "--network=immich" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  # Ensure all containers start after the network is created
  systemd.services.docker-immich-server.after = [ "docker-network-immich.service" ];
  systemd.services.docker-immich-server.requires = [ "docker-network-immich.service" ];
  systemd.services.docker-immich-machine-learning.after = [ "docker-network-immich.service" ];
  systemd.services.docker-immich-machine-learning.requires = [ "docker-network-immich.service" ];
  systemd.services.docker-immich-redis.after = [ "docker-network-immich.service" ];
  systemd.services.docker-immich-redis.requires = [ "docker-network-immich.service" ];
  systemd.services.docker-immich-postgres.after = [ "docker-network-immich.service" ];
  systemd.services.docker-immich-postgres.requires = [ "docker-network-immich.service" ];
}
