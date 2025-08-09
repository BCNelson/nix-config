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
        enabled = true;
        host = "homeassistant.h.b.nel.family";
        port = 1883;
        user = "frigate";
        password = "{FRIGATE_HA_USER_PASSWORD}";
      };
      cameras = {
        doorbell = {
          ffmpeg = {
            inputs = [
              {
                path = "rtsp://127.0.0.1:8554/doorbell";
                roles = [ "record" ];
              }
              {
                path = "rtsp://127.0.0.1:8554/doorbell_sub";
                roles = [ "detect" ];
              }
            ];
          };
          motion.mask = ["0.25,0,0.735,0,0.735,0.07,0.25,0.07"];
        };
        playroom = {
          ffmpeg = {
            inputs = [
              {
                path = "rtsp://127.0.0.1:8554/playroom";
                roles = [ "record" ];
              }
              {
                path = "rtsp://127.0.0.1:8554/playroom_sub";
                roles = [ "detect" ];
              }
            ];
          };
          motion.mask = ["0.25,0,0.735,0,0.735,0.07,0.25,0.07"];
        };
      };
    };
  };

  services.go2rtc = {
    enable = true;
    settings = {
      streams = {
          doorbell = [
            "ffmpeg:http://192.168.3.69/flv?port=1935&app=bcs&stream=channel0_main.bcs&user=service&password=\${CAMERA_PASSWORD}#video=copy#audio=copy#audio=opus"
          ];
          doorbell_sub = [
            "ffmpeg:http://192.168.3.69/flv?port=1935&app=bcs&stream=channel0_ext.bcs&user=service&password=\${CAMERA_PASSWORD}"
          ];
          playroom = [
            "ffmpeg:http://192.168.3.72/flv?port=1935&app=bcs&stream=channel0_main.bcs&user=service&password=\${CAMERA_PASSWORD}#video=copy#audio=copy#audio=opus"
          ];
          playroom_sub = [
            "ffmpeg:http://192.168.3.72/flv?port=1935&app=bcs&stream=channel0_ext.bcs&user=service&password=\${CAMERA_PASSWORD}"
          ];
        };
      rtsp.listen = ":8554";
      webrtc.listen = ":8555";
    };
  };

# TODO: Rename this to camera-password
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
      FRIGATE_HA_USER_PASSWORD=$FRIGATE_HA_USER_PASSWORD
    '';
  };

  age-template.files.go2rtc-env = {
    vars = {
      CAMERA_PASSWORD = config.age.secrets.frigate-camera-password.path;
    };
    content = ''
      CAMERA_PASSWORD=$CAMERA_PASSWORD
    '';
  };

  systemd.services.go2rtc = {
    serviceConfig = {
      EnvironmentFile = config.age-template.files.go2rtc-env.path;
    };
  };

  systemd.services.frigate.serviceConfig= {
    EnvironmentFile = config.age-template.files.frigate-env.path;
    SupplementaryGroups = ["render" "video"] ; # for access to dev/dri/*
    AmbientCapabilities = "CAP_PERFMON";
  };

  services.nginx.virtualHosts."${config.services.frigate.hostname}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
  };
}
