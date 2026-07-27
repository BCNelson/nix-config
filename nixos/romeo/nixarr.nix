{ config, pkgs, lib, libx, inputs, ... }:
let
  dataDirs = config.data.dirs;

  wgConfig = libx.getSecret ./sensitive.nix "airdnsWGConfig";
  wgConfigText = pkgs.writeTextDir "wg.conf" wgConfig;
  peerPort = libx.getSecretWithDefault ./sensitive.nix "airdnsPeerPort" 0;
  # Drop these once the nixarr dev branch ships Jellyfin 10.11.9/10.11.11
  # OpenAPI hashes and a pname matching its pyproject metadata. The nixpkgs
  # bump in flake.lock moved Jellyfin forward first, and added
  # pythonMetadataCheckHook, which rejects the upstream pname/metadata
  # mismatch ("nixarr" vs "nixarr_py").
  patchedNixarrSource = pkgs.runCommandLocal "nixarr-jellyfin-openapi-hash-fix" {} ''
    cp -r ${inputs.nixarr} $out
    chmod -R u+w $out
    substituteInPlace $out/nixarr/lib/nixarr-py/python-deps.nix \
      --replace-fail \
      '"10.11.8" = "sha256-Fqzv/r1ntNn9/wPSD1wRvH9rUyjjBV0lrxw3hdBgrtA=";' \
      '"10.11.8" = "sha256-Fqzv/r1ntNn9/wPSD1wRvH9rUyjjBV0lrxw3hdBgrtA=";
          "10.11.9" = "sha256-3+QrbX658CN46/qfAh3Yj7sRDn50fMlLQvckSHTVuFk=";
          "10.11.10" = "sha256-3FfqhqQfuQdM/02NyhAWDW7H6OaTynWtaUBoSIxk4AQ=";
          "10.11.11" = "sha256-4p/DaeyuVGdsrrUMu8AGtcTulZkGwA8eAvb4PbnCJ/s=";'
    substituteInPlace $out/nixarr/lib/nixarr-py/default.nix \
      --replace-fail 'pname = "nixarr";' 'pname = "nixarr_py";'
  '';

  # nixarr's <arr>-api units wait for their service by curling its *root* with
  # `curl --fail` (waitForService in nixarr/lib/utils.nix). That assumes the
  # root answers <400. Sonarr with AuthenticationMethod=Basic answers 401, so
  # the probe loops forever: sonarr-api sat in start-pre for 2h37m and wedged
  # switch-to-configuration, blocking every deploy to romeo until auto-update's
  # 6h TimeoutStartSec killed it. radarr/lidarr/prowlarr only escape it because
  # forms auth 302s to /login.
  #
  # /ping is the *arr health endpoint and is unauthenticated regardless of the
  # auth method, so probe that instead. Drop this once nixarr stops probing the
  # root upstream.
  arrsWithApiUnits = [ "sonarr" "radarr" "lidarr" "prowlarr" ];
  mkWaitForArrPing = name:
    let
      url = "http://127.0.0.1:${toString config.nixarr.${name}.port}/ping";
    in
    pkgs.writeShellScript "wait-for-${name}-ping" ''
      while ! ${lib.getExe pkgs.curl} \
          --silent \
          --fail \
          --max-time 5 \
          --output /dev/null \
          '${url}'; do
        echo "Waiting for ${name} at '${url}'..."
        sleep 5
      done
      echo "${name} is available at '${url}'"
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

  # See mkWaitForArrPing above: replace nixarr's root probe with a /ping probe.
  systemd.services = lib.genAttrs (map (name: "${name}-api") arrsWithApiUnits) (
    unit: {
      serviceConfig.ExecStartPre = lib.mkForce [
        (mkWaitForArrPing (lib.removeSuffix "-api" unit))
      ];
    }
  );

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
