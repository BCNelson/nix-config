{ config, libx, ... }:
let
  dataDirs = config.data.dirs;
  sensitiveData = libx.getSecretWithDefault ../../sensitive.nix "foundry" {
    account_password = "";
    admin_password = "";
  };
in
{
  virtualisation.oci-containers.containers.foundryvtt = {
    image = "docker.io/felddy/foundryvtt:13";
    hostname = "pathfinder.h.b.nel.family";
    ports = [ "127.0.0.1:30000:30000" ];
    volumes = [ "${dataDirs.level4}/foundryvtt/data:/data" ];
    environment = {
      FOUNDRY_USERNAME = "bcnelson";
      FOUNDRY_PASSWORD = sensitiveData.account_password;
      CONTAINER_CACHE = "/data/container_cache";
      FOUNDRY_ADMIN_KEY = sensitiveData.admin_password;
      FOUNDRY_MINIFY_STATIC_FILES = "true";
      FOUNDRY_HOSTNAME = "pathfinder.h.b.nel.family";
      FOUNDRY_PROXY_PORT = "443";
      FOUNDRY_PROXY_SSL = "true";
      TIMEZONE = "America/Denver";
      FOUNDRY_WORLD = "Lambda";
    };
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}
