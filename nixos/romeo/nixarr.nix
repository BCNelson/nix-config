{ config, pkgs, libx, inputs, ... }:
let
  dataDirs = config.data.dirs;

  wgConfig = libx.getSecret ./sensitive.nix "airdnsWGConfig";
  wgConfigText = pkgs.writeTextDir "wg.conf" wgConfig;
  peerPort = libx.getSecretWithDefault ./sensitive.nix "airdnsPeerPort" 0;
  # Drop this once the nixarr dev branch ships Jellyfin 10.11.9/10.11.10
  # OpenAPI hashes. The nixpkgs bump in flake.lock moved Jellyfin forward first.
  patchedNixarrSource = pkgs.runCommandLocal "nixarr-jellyfin-openapi-hash-fix" {} ''
    cp -r ${inputs.nixarr} $out
    chmod -R u+w $out
    substituteInPlace $out/nixarr/lib/nixarr-py/python-deps.nix \
      --replace-fail \
      '"10.11.8" = "sha256-Fqzv/r1ntNn9/wPSD1wRvH9rUyjjBV0lrxw3hdBgrtA=";' \
      '"10.11.8" = "sha256-Fqzv/r1ntNn9/wPSD1wRvH9rUyjjBV0lrxw3hdBgrtA=";
          "10.11.9" = "sha256-3+QrbX658CN46/qfAh3Yj7sRDn50fMlLQvckSHTVuFk=";
          "10.11.10" = "sha256-3FfqhqQfuQdM/02NyhAWDW7H6OaTynWtaUBoSIxk4AQ=";'
  '';
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
    nixarr-py.package = pkgs.callPackage "${patchedNixarrSource}/nixarr/lib/nixarr-py" {};

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
