{ config, ... }:
{
  age.secrets.immichframe-api-key = {
    rekeyFile = ../../../secrets/store/romeo/immichframe_api_key.age;
  };

  age-template.files.immichframe-env = {
    vars = {
      API_KEY = config.age.secrets.immichframe-api-key.path;
    };
    content = ''
      ApiKey=$API_KEY
    '';
  };

  virtualisation.oci-containers.containers.immichframe = {
    image = "ghcr.io/immichframe/immichframe:latest";
    environment = {
      "TZ" = "America/Denver";
      "ImmichServerUrl" = "http://immich-server:2283";
      "Interval" = "45";
      "TransitionDuration" = "2";
      "ShowClock" = "true";
      "ShowPhotoDate" = "true";
      "ShowImageDesc" = "true";
      "ShowImageLocation" = "true";
      "ShowMemories" = "true";
    };
    environmentFiles = [
      "${config.age-template.files.immichframe-env.path}"
    ];
    ports = [ "0.0.0.0:8088:8080" ];
    dependsOn = [ "immich-server" ];
    extraOptions = [ "--network=immich" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  systemd.services.docker-immichframe.after = [ "docker-network-immich.service" ];
  systemd.services.docker-immichframe.requires = [ "docker-network-immich.service" ];
}
