{ dataDirs, libx, pkgs }:
let
  steam_api_key = libx.getSecret ../../sensitive.nix "steam_api_key";

  config = pkgs.writeText "config.yaml" (builtins.toJSON {
    steamKey = steam_api_key;
    pinnedPackages = {
      ssri = "8.0.1";
      cacache = "16.1.3";
      passport-steam = "1.0.17";
      minipass-fetch = "3.0.3";
    };
    mods = [
      "screepsmod-auth"
      "screepsmod-admin-utils"
      "screepsmod-map-tool"
      "screepsmod-history"
      "screepsmod-mongo"
    ];
    serverConfig = {
        tickRate = 1000;
    };
  });
in
{
  screep-server = {
    image = "screepers/screeps-launcher";
    container_name = "screep-server";
    volumes = [
      "${config}:/screeps/config.yml:ro"
      "${dataDirs.level3}/screeps/server:/screeps"
    ];
    environment = [
      "MONGO_HOST=screeps-mongo"
      "REDIS_HOST=screeps-redis"
    ];
    restart = "unless-stopped";
  };
  screeps-redis = {
    image = "redis";
    container_name = "screeps-redis";
    volumes = [
      "${dataDirs.level3}/screeps/redis:/data"
    ];
    restart = "unless-stopped";
  };
  screeps-mongo = {
    image = "mongo";
    container_name = "screeps-mongo";
    volumes = [
      "${dataDirs.level3}/screeps/mongo:/data/db"
    ];
    restart = "unless-stopped";
  };
}
