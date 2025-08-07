{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  # Init container to set proper permissions on mounted volumes
  virtualisation.oci-containers.containers.magicmirror-init = {
    image = "karsten13/magicmirror:latest";
    user = "root";
    environment = {
      "STARTENV" = "init";
      "MM_UID" = "1000";
      "MM_GID" = "1000";
      "MM_CHMOD" = "777";
    };
    volumes = [
      "${dataDirs.level5}/magicmirror/config:/opt/magic_mirror/config"
      "${dataDirs.level5}/magicmirror/modules:/opt/magic_mirror/modules"
      "${dataDirs.level5}/magicmirror/css:/opt/magic_mirror/css"
    ];
    autoStart = false;  # Run manually when needed
  };

  # Main Magic Mirror container
  virtualisation.oci-containers.containers.magicmirror = {
    image = "karsten13/magicmirror:latest";
    environment = {
      "MM_SCENARIO" = "server";
      "MM_MODULES_DIR" = "modules";
      "MM_CUSTOMCSS_FILE" = "css/custom.css";
      "MM_OVERRIDE_DEFAULT_MODULES" = "true";
      "MM_SHOW_CURSOR" = "false";
    };
    volumes = [
      "${dataDirs.level5}/magicmirror/config:/opt/magic_mirror/config"
      "${dataDirs.level5}/magicmirror/modules:/opt/magic_mirror/modules"
      "${dataDirs.level5}/magicmirror/css/custom.css:/opt/magic_mirror/css/custom.css"
    ];
    ports = [ "8085:8080" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  # Magic Mirror Package Manager (MMPM) - optional package manager
  virtualisation.oci-containers.containers.mmpm = {
    image = "karsten13/mmpm:latest";
    dependsOn = [ "magicmirror" ];
    ports = [
      "7890:7890"
      "7891:7891" 
      "6789:6789"
      "8907:8907"
    ];
    volumes = [
      "${dataDirs.level5}/magicmirror/modules:/home/node/MagicMirror/modules"
      "${dataDirs.level5}/magicmirror/config:/home/node/MagicMirror/config"
      "${dataDirs.level5}/magicmirror/css/custom.css:/home/node/MagicMirror/css/custom.css"
      "${dataDirs.level5}/mmpm/config:/home/node/.config/mmpm"
    ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}