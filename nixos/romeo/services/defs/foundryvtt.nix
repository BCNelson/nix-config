{ dataDirs, libx }:
let
  sensitiveData = libx.getSecret ../../../sensitive.nix "foundry";
in
{
  foundryvtt = {
    image = "felddy/foundryvtt:release";
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
    ];
    volumes = [
      "${dataDirs.level4}/foundryvtt/data:/data"
    ];
    restart = "unless-stopped";
  };
}
