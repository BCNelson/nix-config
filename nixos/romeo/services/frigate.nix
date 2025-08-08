{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  services.frigate = {
    enable = true;
    hostname = "frigate.h.b.nel.family";
    settings = {
      database.path = "${dataDirs.level5}/frigate/db/frigate.db";
      mqtt = {
        host = "homeassistant.h.b.nel.family";
        port = 1883;
        user = "frigate";
        password = "{FRIGATE_HA_USER_PASSWORD}";
      };
    };
  };

  age.secrets.frigate-camera-password = {
    rekeyFile = ./secrets/frigate_camera_password.age;
  };

  age.secrets.frigate-ha-user-password = {
    rekeyFile = ./secrets/frigate_ha_user_password.age;
  };

  age-template.files.frigate-env = {
    vars = {
      FRIGATE_CAMERA_PASSWORD = config.age.secrets.frigate-camera-password.path;
      FRIGATE_HA_USER_PASSWORD = config.age.secrets.frigate-ha-user-password.path;
    };
    content = ''
      FRIGATE_CAMERA_PASSWORD=$FRIGATE_CAMERA_PASSWORD
      FRIGATE_HA_USER_PASSWORD=$FRIGATE_HA_USER_PASSWORD
    '';
  };

  systemd.services.frigate.serviceConfig.EnvironmentFile = config.age-template.files.frigate-env.path;

  services.nginx.virtualHosts."${config.services.frigate.hostname}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
  };
}
