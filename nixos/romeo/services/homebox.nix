{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  # homebox 0.26.0 made an API-key pepper mandatory; without it the container
  # panics on startup ("auth.api_key_pepper must be set to at least 32 bytes")
  # and restart-loops into start-limit-hit. Rotating this value only invalidates
  # already-issued API keys -- it does not touch user data or logins -- but
  # there is no reason to churn it, so it is generated once and kept.
  age.secrets.homebox-api-key-pepper = {
    rekeyFile = ./secrets/homebox_api_key_pepper.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -base64 48";
  };

  age-template.files.homebox-env = {
    vars = {
      API_KEY_PEPPER = config.age.secrets.homebox-api-key-pepper.path;
    };
    content = ''
      HBOX_AUTH_API_KEY_PEPPER=$API_KEY_PEPPER
    '';
  };

  virtualisation.oci-containers.containers.homebox = {
    # Pinned to the minor series: on ":latest" the 0.26 breaking change above
    # was pulled in June and only detonated in July, when an unrelated deploy
    # recreated every container at once.
    image = "ghcr.io/sysadminsmedia/homebox:0.26";
    ports = [ "127.0.0.1:7745:7745" ];
    volumes = [ "${dataDirs.level3}/homeBox:/data/" ];
    environment = {
      HBOX_LOG_LEVEL = "info";
      HBOX_LOG_FORMAT = "text";
      HBOX_WEB_MAX_UPLOAD_SIZE = "50";
      HBOX_OPTIONS_ALLOW_REGISTRATION = "false";
    };
    environmentFiles = [
      "${config.age-template.files.homebox-env.path}"
    ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
}
