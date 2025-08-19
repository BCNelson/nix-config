{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  services.frigate = {
    enable = true;
    checkConfig = false;
    hostname = "frigate.h.b.nel.family";
    settings = {
      database.path = "${dataDirs.level5}/frigate/db/frigate.db";
      mqtt = {
        enabled = true;
        host = "192.168.3.6";
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
      detectors = {
        ov = {
          type = "openvino";
          device = "GPU";
        };
      };
      record = {
        enabled = true;
        retain = {
          days = 7;
          mode = "motion";
        };
      };
      snapshots = {
        enabled = true;
        retain = {
          default = 30;
        };
      };
      model = {
        model_type = "yolonas";
        width = 320;
        height = 320;
        input_tensor = "nchw";
        input_pixel_format = "bgr";
        path = "${dataDirs.level5}/frigate/model/yolo_nas_s.onnx";
        labelmap_path = "${dataDirs.level5}/frigate/model/labelmap.txt";
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
            "ffmpeg:http://192.168.3.80/flv?port=1935&app=bcs&stream=channel0_main.bcs&user=service&password=\${CAMERA_PASSWORD}#video=copy#audio=copy#audio=opus"
          ];
          playroom_sub = [
            "ffmpeg:http://192.168.3.80/flv?port=1935&app=bcs&stream=channel0_ext.bcs&user=service&password=\${CAMERA_PASSWORD}"
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

  age.secrets.frigate-plus-api-key = {
    rekeyFile = ./secrets/frigate_plus_api_key.age;
  };

  age-template.files.frigate-env = {
    vars = {
      FRIGATE_CAMERA_PASSWORD = config.age.secrets.frigate-camera-password.path;
      FRIGATE_HA_USER_PASSWORD = config.age.secrets.frigate-ha-user-password.path;
      API_KEY = config.age.secrets.frigate-plus-api-key.path;
    };
    content = ''
      FRIGATE_HA_USER_PASSWORD=$FRIGATE_HA_USER_PASSWORD
      PLUS_API_KEY=$API_KEY
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
