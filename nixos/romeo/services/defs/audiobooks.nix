{ dataDirs }:
{
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
      "${dataDirs.level6}/media/readarr:/readarr"
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
}
