{ config, pkgs, lib, libx, inputs, ... }:
let
  dataDirs = config.data.dirs;

  wgConfig = libx.getSecret ./sensitive.nix "airdnsWGConfig";
  wgConfigText = pkgs.writeTextDir "wg.conf" wgConfig;
  peerPort = libx.getSecretWithDefault ./sensitive.nix "airdnsPeerPort" 0;
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

  # Forward auth against authentik on whiskey (provider + embedded outpost in
  # nixos/whiskey/services/authentik/blueprints/arr.yaml, which also limits
  # access to service_admins). Every request to "/" is checked with an
  # auth_request to the outpost; a 401 bounces the browser to the outpost's
  # /start, which runs the authentik login and returns here.
  #
  # The outpost is reached by name at request time (resolver + variable) rather
  # than resolved at config load, so nginx still starts if unbound is not up yet
  # or whiskey is unreachable; only the arr vhosts fail until it is.
  arrPorts = {
    prowlarr = 9696;
    radarr = 7878;
    sonarr = 8989;
    lidarr = 8686;
  };
  authentikHost = "auth.nel.family";
  mkArrVhost = port: {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;
    '';
    locations = {
      "/" = {
        proxyPass = "http://localhost:${toString port}";
        extraConfig = ''
          proxy_max_temp_file_size 2048m;

          auth_request /outpost.goauthentik.io/auth/nginx;
          error_page 401 = @goauthentik_proxy_signin;
          auth_request_set $auth_cookie $upstream_http_set_cookie;
          add_header Set-Cookie $auth_cookie;
        '';
      };
      # API calls skip SSO: the apps still require their API key here, and this
      # keeps mobile clients and API-key integrations working. The apps' own
      # traffic to each other goes over localhost and never touches nginx.
      "~ ^/api(/|$)" = {
        proxyPass = "http://localhost:${toString port}";
        extraConfig = ''
          proxy_max_temp_file_size 2048m;
        '';
      };
      "/outpost.goauthentik.io" = {
        # recommendedProxySettings is included after extraConfig and would
        # re-set Host to the arr vhost, which whiskey's nginx would route to
        # the wrong server block.
        recommendedProxySettings = false;
        extraConfig = ''
          resolver 127.0.0.1 valid=300s;
          set $authentik https://${authentikHost};
          proxy_pass $authentik;
          proxy_ssl_server_name on;
          proxy_set_header Host ${authentikHost};
          proxy_set_header X-Original-URL $scheme://$http_host$request_uri;
          proxy_pass_request_body off;
          proxy_set_header Content-Length "";
          auth_request_set $auth_cookie $upstream_http_set_cookie;
          add_header Set-Cookie $auth_cookie;
        '';
      };
      "@goauthentik_proxy_signin".extraConfig = ''
        internal;
        add_header Set-Cookie $auth_cookie;
        return 302 /outpost.goauthentik.io/start?rd=$scheme://$http_host$request_uri;
      '';
    };
  };
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

  # The apps trust nginx's forward auth (below) instead of running their own
  # login, so authentik is the only prompt. Their ports are not in the firewall,
  # so nginx is the only way in from off-host.
  services.sonarr.settings.auth.method = "External";
  services.radarr.settings.auth.method = "External";
  services.lidarr.settings.auth.method = "External";
  services.prowlarr.settings.auth.method = "External";

  services.nginx = {
    enable = true;
    virtualHosts = lib.mapAttrs' (name: port:
      lib.nameValuePair "${name}.arr.b.nel.family" (mkArrVhost port)
    ) arrPorts;
  };

  services.jackett = {
    enable = true;
    dataDir = "${dataDirs.level6}/jackett";
    openFirewall = true;
  };
}
