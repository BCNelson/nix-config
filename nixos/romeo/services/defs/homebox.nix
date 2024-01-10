{ dataDirs }:
{
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
}
