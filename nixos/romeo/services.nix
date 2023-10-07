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
        };
    };
  };
}
