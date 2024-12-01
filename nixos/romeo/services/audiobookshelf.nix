{ config, ... }: let
    dataDirs = config.data.dirs;
in{
    virtualisation.oci-containers.containers.audiobookshelf = {
        image = "docker.io/audiobookshelf/audiobookshelf:latest";
        environment = {
            "AUDIOBOOKSHELF_UID" = "99";
            "AUDIOBOOKSHELF_GID" = "100";
        };
        volumes = [
            "${dataDirs.level6}/media/audiobooks:/audiobooks"
            "${dataDirs.level6}/media/audible:/audible"
            "${dataDirs.level6}/media/audible3:/audible3"
            "${dataDirs.level6}/media/podcasts:/podcasts"
            "${dataDirs.level6}/media/readarr:/readarr"
            "${dataDirs.level5}/audiobookshelf/config:/config"
            "${dataDirs.level5}/audiobookshelf/metadata:/metadata"
        ];
        ports = [ "127.0.0.1:8080:80" ];
    };
}