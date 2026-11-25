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

  # Forward auth against authentik on whiskey (see ./authentik-forward-auth.nix;
  # the provider in nixos/whiskey/services/authentik/blueprints/arr.yaml limits
  # access to service_admins).
  arrPorts = {
    prowlarr = 9696;
    radarr = 7878;
    sonarr = 8989;
    lidarr = 8686;
  };
  withAuthentik = import ./authentik-forward-auth.nix;
  mkArrVhost = { port, apiBypass ? true }: {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    # Tailnet only. DNS for these names points at romeo's Tailscale address
    # (main.tf), and the h.b ingress is a plain port forward that preserves the
    # client's address, so anything not from Tailscale's ranges -- the internet,
    # or a LAN client without Tailscale -- is refused before SSO even runs.
    extraConfig = ''
      client_max_body_size 0;

      allow 100.64.0.0/10;
      allow fd7a:115c:a1e0::/48;
      deny all;
    '';
    locations = withAuthentik (lib.optionalAttrs apiBypass {
      # API calls skip SSO: the apps still require their API key here, and this
      # keeps mobile clients and API-key integrations working. The apps' own
      # traffic to each other goes over localhost and never touches nginx.
      "~ ^/api(/|$)" = {
        proxyPass = "http://localhost:${toString port}";
        extraConfig = ''
          proxy_max_temp_file_size 2048m;
        '';
      };
    } // {
      "/" = {
        proxyPass = "http://localhost:${toString port}";
        extraConfig = ''
          proxy_max_temp_file_size 2048m;
        '';
      };
    });
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
  ) // {
    # The widgets need each app's API key, which nixarr's <arr>-api units export
    # to ${stateDir}/secrets/<arr>.api-key (root:<arr>-api, 0640). homepage runs
    # with DynamicUser + PrivateUsers, so it cannot read those itself. This
    # root oneshot gathers them into an env file that systemd (as root) feeds
    # to homepage as HOMEPAGE_VAR_*.
    #
    # Not HOMEPAGE_FILE_*: homepage pastes file contents into the YAML
    # verbatim, and the exported keys end in a newline, which broke the quoted
    # `key:` scalar and made homepage print the key into the journal.
    homepage-arr-keys = {
      description = "Collect arr API keys for homepage";
      after = map (name: "${name}-api.service") arrsWithApiUnits;
      wants = map (name: "${name}-api.service") arrsWithApiUnits;
      before = [ "homepage-dashboard.service" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        RuntimeDirectory = "homepage-arr-keys";
        RuntimeDirectoryMode = "0700";
        UMask = "0077";
      };
      script = ''
        out=/run/homepage-arr-keys/env
        : > "$out.tmp"
        ${lib.concatMapStrings (name: ''
          printf 'HOMEPAGE_VAR_${lib.toUpper name}_KEY=%s\n' \
            "$(tr -d '[:space:]' < '${config.nixarr.stateDir}/secrets/${name}.api-key')" >> "$out.tmp"
        '') arrsWithApiUnits}
        mv "$out.tmp" "$out"
      '';
    };
    homepage-dashboard = {
      requires = [ "homepage-arr-keys.service" ];
      after = [ "homepage-arr-keys.service" ];
    };
  };

  # Dashboard for the whole stack at arr.b.nel.family, behind the same
  # authentik forward auth as the apps. Widgets talk to the apps over localhost;
  # the links are what the browser opens.
  services.homepage-dashboard = {
    enable = true;
    # The module default, 8082, is Frigate's internal port on romeo.
    listenPort = 8084;
    # Written by homepage-arr-keys (above); systemd reads it as root.
    environmentFiles = [ "/run/homepage-arr-keys/env" ];
    allowedHosts = "arr.b.nel.family";
    settings = {
      title = "Media";
      theme = "dark";
      color = "slate";
      headerStyle = "clean";
      layout = {
        Watch = { style = "row"; columns = 2; };
        Automation = { style = "row"; columns = 4; };
        Downloads = { style = "row"; columns = 2; };
      };
    };
    widgets = [
      { resources = { cpu = true; memory = true; disk = config.nixarr.mediaDir; }; }
    ];
    services =
      let
        arrTile = name: description: {
          ${lib.toSentenceCase name} = {
            inherit description;
            href = "https://${name}.arr.b.nel.family/";
            icon = "${name}.svg";
            widget = {
              type = name;
              url = "http://127.0.0.1:${toString arrPorts.${name}}";
              key = "{{HOMEPAGE_VAR_${lib.toUpper name}_KEY}}";
            };
          };
        };
      in
      [
        {
          Watch = [
            {
              Jellyfin = {
                description = "Stream the library";
                href = "https://media.nel.family/";
                icon = "jellyfin.svg";
              };
            }
          ];
        }
        {
          Automation = [
            (arrTile "sonarr" "TV")
            (arrTile "radarr" "Movies")
            (arrTile "lidarr" "Music")
            (arrTile "prowlarr" "Indexers")
          ];
        }
        {
          Downloads = [
            {
              Transmission = {
                description = "Torrents, over the VPN";
                href = "http://romeo.b.nel.family:${toString config.nixarr.transmission.uiPort}/";
                icon = "transmission.svg";
                # RPC listens inside the VPN namespace; its address is reachable
                # from the host through the namespace bridge. No RPC auth is set.
                widget = {
                  type = "transmission";
                  url = "http://${config.vpnNamespaces.wg.namespaceAddress}:${toString config.nixarr.transmission.uiPort}";
                  rpcUrl = "/transmission/";
                };
              };
            }
          ];
        }
      ];
  };

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
      lib.nameValuePair "${name}.arr.b.nel.family" (mkArrVhost { inherit port; })
    ) arrPorts // {
      # No /api bypass here: homepage's /api is the proxy that calls the apps
      # *with* their API keys, so it has to stay behind SSO.
      "arr.b.nel.family" = mkArrVhost {
        port = config.services.homepage-dashboard.listenPort;
        apiBypass = false;
      };
    };
  };

  services.jackett = {
    enable = true;
    dataDir = "${dataDirs.level6}/jackett";
    openFirewall = true;
  };
}
