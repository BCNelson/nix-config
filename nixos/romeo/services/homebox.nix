{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  virtualisation.oci-containers.containers.homebox = {
    image = "ghcr.io/sysadminsmedia/homebox:latest";
    ports = [ "127.0.0.1:7745:7745" ];
    volumes = [ "${dataDirs.level3}/homeBox:/data/" ];
    environment = {
      HBOX_LOG_LEVEL = "info";
      HBOX_LOG_FORMAT = "text";
      HBOX_WEB_MAX_UPLOAD_SIZE = "50";
      HBOX_OPTIONS_ALLOW_REGISTRATION = "false";
    };
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}
