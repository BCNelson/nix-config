{ dataDirs, libx }:
let
  sensitiveData = libx.getSecretWithDefault ../../../sensitive.nix "foundry" {
    account_password = "";
    admin_password = "";
  };
in
{
  foundryvtt = {
    image = "felddy/foundryvtt:12";
    container_name = "foundryvtt";
    environment = [
      "FOUNDRY_USERNAME=bcnelson"
      "FOUNDRY_PASSWORD=${sensitiveData.account_password}"
      "CONTAINER_CACHE=/data/container_cache"
      "FOUNDRY_ADMIN_KEY=${sensitiveData.admin_password}"
      "FOUNDRY_MINIFY_STATIC_FILES=true"
      "FOUNDRY_HOSTNAME=pathfinder.h.b.nel.family"
      "FOUNDRY_PROXY_PORT=443"
      "FOUNDRY_PROXY_SSL=true"
      "TIMEZONE=America/Denver"
      "FOUNDRY_WORLD=Lambda"
    ];
    volumes = [
      "${dataDirs.level4}/foundryvtt/data:/data"
    ];
    ports = [ "127.0.0.1:30000:30000" ];
    restart = "unless-stopped";
  };
}
