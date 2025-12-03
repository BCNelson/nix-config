{ config, pkgs, libx, inputs, ... }:
let
  dataDirs = config.data.dirs;

  wgConfig = libx.getSecret ./sensitive.nix "airdnsWGConfig";
  wgConfigText = pkgs.writeTextDir "wg.conf" wgConfig;
  peerPort = libx.getSecretWithDefault ./sensitive.nix "airdnsPeerPort" 0;
in
{
  imports =
    [
      inputs.nixarr.nixosModules.default
    ];

  nixpkgs.config.permittedInsecurePackages = [
    "dotnet-sdk-6.0.428"
    "aspnetcore-runtime-6.0.36"
  ];

  nixarr = {
    enable = true;

    vpn = {
      enable = true;
      wgConf = "${wgConfigText}/wg.conf";
    };

    stateDir = "${dataDirs.level4}/nixarr";
    mediaDir = "${dataDirs.level6}/media";

    transmission = {
      enable = true;
      vpn.enable = true;
      inherit peerPort;
      flood.enable = true;
      extraAllowedIps = [ "100.*.*.*" ];
      extraSettings = {
        "rpc-host-whitelist" = "romeo.b.nel.family";
        "rpc-host-whitelist-enabled" = true;
        "download_queue_enabled" = true;
        "download_queue_size" = 50;
        "queue_stalled_enabled" = true;
        "queue_stalled_minutes" = 5;
      };
    };

    bazarr.enable = true;
    prowlarr.enable = true;
    radarr.enable = true;
    readarr.enable = true;
    sonarr.enable = true;
    lidarr.enable = true;
  };

  services.flaresolverr.enable = true;

  services.nginx = {
    enable = true;
    virtualHosts = {
      "bazarr.arr.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:6767";
            extraConfig = ''
              proxy_max_temp_file_size 2048m;
            '';
          };
        };
      };
      "prowlarr.arr.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:9696";
            extraConfig = ''
              proxy_max_temp_file_size 2048m;
            '';
          };
        };
      };
      "readarr.arr.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8787";
            extraConfig = ''
              proxy_max_temp_file_size 2048m;
            '';
          };
        };
      };
      "radarr.arr.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:7878";
            extraConfig = ''
              proxy_max_temp_file_size 2048m;
            '';
          };
        };
      };
      "sonarr.arr.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8989";
            extraConfig = ''
              proxy_max_temp_file_size 2048m;
            '';
          };
        };
      };
      "lidarr.arr.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:8686";
            extraConfig = ''
              proxy_max_temp_file_size 2048m;
            '';
          };
        };
      };
    };
  };

  services.jackett = {
    enable = true;
    dataDir = "${dataDirs.level6}/jackett";
    openFirewall = true;
  };
}
