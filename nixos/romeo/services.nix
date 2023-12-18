{ libx, dataDirs, pkgs, ... }:
let
  sensitiveData = import ../sensitive.nix;
  linodeToken = pkgs.writeTextFile {
    name = "linode-dns-config";
    text = ''
      dns_linode_key = ${sensitiveData.dns_linode_key}
    '';
    destination = "/linode.ini";
  };
  config = ".";
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./config;
    dependencies = [ linodeToken ];
    dockerComposeDefinition = {
      version = "3.8";
      services = {
        swag = {
          build = "./swag";
          container_name = "swag";
          cap_add = [ "NET_ADMIN" ];
          environment = [
            "PUID=1002"
            "PGID=1002"
            "TZ=America/Denver"
            "URL=h.b.nel.family"
            "SUBDOMAINS=wildcard"
            "VALIDATION=dns"
            "DNSPLUGIN=linode"
            "EMAIL=bradley@nel.family"
            "DHLEVEL=2048"
            "ONLY_SUBDOMAINS=true"
            "STAGING=false"
            "EXTRA_DOMAINS= *.romeo.b.nel.family, *.nel.family *.bnel.me nel.to"
            "DOCKER_MODS=linuxserver/mods:swag-auto-reload"
          ];
          volumes = [
            "${dataDirs.level7}/swag:/config"
            "${linodeToken}/linode.ini:/config/dns-conf/linode.ini:ro"
            "${config}/swag/nginx/proxy-confs:/config/nginx/proxy-confs:ro"
            "${config}/swag/nginx/tailscale.conf:/config/nginx/tailscale.conf:ro"
            "${config}/swag/nginx/internal.conf:/config/nginx/internal.conf:ro"
          ];
          ports = [
            "443:443"
            "80:80"
          ];
          restart = "unless-stopped";
          # networks = [ "external" ]; # TODO: figure out how to make network definion cleaner
        };
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
        audiobookshelf = {
          image = "advplyr/audiobookshelf";
          environment = [
            "AUDIOBOOKSHELF_UID=99"
            "AUDIOBOOKSHELF_GID=100"
          ];
          volumes = [
            "${dataDirs.level6}/media/audiobooks:/audiobooks"
            "${dataDirs.level6}/media/audible:/audible"
            "${dataDirs.level6}/media/audible3:/audible3"
            "${dataDirs.level5}/audiobookshelf/config:/config"
            "${dataDirs.level5}/audiobookshelf/metadata:/metadata"
          ];
          container_name = "audiobookshelf";
          restart = "unless-stopped";
        };
        openAudible = {
          image = "openaudible/openaudible:latest";
          container_name = "openaudible";
          volumes = [
            "${dataDirs.level5}/openAudible:/config/OpenAudible"
            "${dataDirs.level6}/media/audible:/media/audiobooks"
          ];
          restart = "unless-stopped";
        };
        libation = {
          image = "ghcr.io/bcnelson/libation_docker:latest";
          container_name = "libation";
          volumes = [
            "${dataDirs.level5}/libation:/config/Libation"
            "${dataDirs.level6}/media/audible3:/media/audiobooks"
          ];
          restart = "unless-stopped";
        };
        nextcloud = {
          image = "ghcr.io/linuxserver/nextcloud:latest";
          container_name = "nextcloud";
          environment = [
            "PUID=1000"
            "PGID=1000"
            "TZ=America/Denver"
          ];
          volumes = [
            "${dataDirs.level5}/nextcloud/config:/config"
            "${dataDirs.level5}/nextcloud/data:/data"
          ];
          links = [ "nextcloud_db:db" ];
          restart = "unless-stopped";
        };
        nextcloud_db = {
          image = "postgres:12";
          restart = "unless-stopped";
          container_name = "nextcloud_db";
          volumes = [
            "${dataDirs.level5}/nextcloud/database:/var/lib/postgresql/data"
          ];
          environment = [
            "POSTGRES_PASSWORD=${sensitiveData.nextcloud_postgress_password}"
          ];
        };
        vikunja_db = {
          image = "postgres:13";
          environment = [
            "POSTGRES_PASSWORD=${sensitiveData.vikunja_postgress_password}"
            "POSTGRES_USER=vikunja"
          ];
          volumes = [
            "${dataDirs.level5}/vikunja/database:/var/lib/postgresql/data"
          ];
          restart = "unless-stopped";
        };
        vikunja_api = {
          image = "vikunja/api";
          environment = [
            "VIKUNJA_DATABASE_HOST=vikunja_db"
            "VIKUNJA_DATABASE_PASSWORD=${sensitiveData.vikunja_postgress_password}"
            "VIKUNJA_DATABASE_TYPE=postgres"
            "VIKUNJA_DATABASE_USER=vikunja"
            "VIKUNJA_DATABASE_DATABASE=vikunja"
            "VIKUNJA_SERVICE_JWTSECRET=${sensitiveData.vikunja_jwt_secret}"
            "VIKUNJA_SERVICE_FRONTENDURL=https://todo.nel.family/"
          ];
          volumes = [
            "${dataDirs.level5}/vikunja/files:/app/vikunja/files"
            "${config}/vikunja/config.yml:/app/vikunja/config.yml"
          ];
          depends_on = [ "vikunja_db" ];
          restart = "unless-stopped";
        };
        vikunja_frontend = {
          image = "vikunja/frontend";
          environment = [
            "VIKUNJA_API_URL=https://todo.nel.family/api/v1"
          ];
          restart = "unless-stopped";
        };
        syncthing = {
          image = "ghcr.io/linuxserver/syncthing";
          container_name = "syncthing";
          environment = [
            "PUID=1000"
            "PGID=1000"
            "TZ=America/Denver"
          ];
          volumes = [
            "${dataDirs.level5}/syncthing/config:/config"
            "${dataDirs.level4}/syncthing/data:/data/folders/level4"
          ];
          ports = [
            "22000:22000/tcp"
            "22000:22000/udp"
            "21027:21027/udp"
          ];
          restart = "unless-stopped";
        };

        foundryvtt = {
          image = "felddy/foundryvtt:release";
          container_name = "foundryvtt";
          environment = [
            "FOUNDRY_USERNAME=bcnelson"
            "FOUNDRY_PASSWORD=${sensitiveData.foundry.account_password}"
            "CONTAINER_CACHE=/data/container_cache"
            "FOUNDRY_ADMIN_KEY=${sensitiveData.foundry.admin_password}"
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
        fastenhealth = {
          image = "ghcr.io/fastenhealth/fasten-onprem:main";
          container_name = "fastenhealth";
          volumes = [
            "${dataDirs.level3}/fastenhealth/db:/opt/fasten/db"
            "${dataDirs.level7}/fastenhealth/cache:/opt/fasten/cache"
          ];
        };
        homeBox = {
          image = "ghcr.io/hay-kot/homebox:latest";
          container_name = "homebox";
          restart = "unless-stopped";
          environment = [
            "HBOX_LOG_LEVEL=info"
            "HBOX_LOG_FORMAT=text"
            "HBOX_WEB_MAX_UPLOAD_SIZE=50"
            "HBOX_OPTIONS_ALLOW_REGISTRATION=false"
          ];
          volumes = [
            "${dataDirs.level3}/homeBox:/data/"
          ];
        };
      };
    };
  };
}
