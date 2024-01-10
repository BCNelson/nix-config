{ dataDirs }:
{
  jellyfin = {
    image = "jellyfin/jellyfin";
    container_name = "jellyfin";
    user = "1000:1000";
    volumes = [
      "${dataDirs.level5}/jellyfin/config:/config"
      "${dataDirs.level7}/jellyfin/cache:/cache"
      "${dataDirs.level6}/media:/media:ro"
    ];
    group_add = [ "303" ];
    devices = [ "/dev/dri:/dev/dri" ];
    restart = "unless-stopped";
    environment = [
      "JELLYFIN_PublishedServerUrl=https://media.nel.family"
    ];
  };
}
