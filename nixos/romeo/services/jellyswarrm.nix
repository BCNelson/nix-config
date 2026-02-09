{ config, pkgs, inputs, lib, ... }:
let
  dataDirs = config.data.dirs;
  # Rebuild jellyswarrm package with fixed cargoHash (upstream has incorrect hash)
  jellyswarrmFixed = pkgs.rustPlatform.buildRustPackage rec {
    pname = "jellyswarrm";
    version = "0.2.0";

    src = pkgs.fetchFromGitHub {
      owner = "LLukas22";
      repo = "Jellyswarrm";
      rev = "v${version}";
      hash = "sha256-bf+HiZLS54abDV9wW/MZQT/UJrtUQMlmFcmN+5T2FYU=";
    };

    cargoHash = "sha256-aWMW/mACrdCQWCi+9+2jQXYYEE1e84xlFWexr+SzM2o=";

    buildInputs = [ pkgs.jellyfin-web ];

    env.JELLYSWARRM_SKIP_UI = "1";

    preBuild = ''
      mkdir -p crates/jellyswarrm-proxy/static
      cp -r ${pkgs.jellyfin-web}/share/jellyfin-web/* crates/jellyswarrm-proxy/static/
      echo "JELLYFIN_WEB_VERSION=${pkgs.jellyfin-web.version}" > crates/jellyswarrm-proxy/static/ui-version.env
    '';

    meta = {
      description = "A reverse proxy for managing multiple Jellyfin servers";
      homepage = "https://github.com/LLukas22/Jellyswarrm";
      license = lib.licenses.gpl3;
      mainProgram = "jellyswarrm";
    };
  };
in
{
  age.secrets.jellyswarrm-password = {
    rekeyFile = ./secrets/jellyswarrm_password.age;
    generator.script = "passphrase";
    mode = "0400";
    owner = "jellyswarrm";
    bitwarden = {
      name = "Jellyswarrm Admin Password";
      username = "admin";
      uris = { uri = "https://jellyswarrm.h.b.nel.family"; matchType = "host"; };
    };
  };

  services.jellyswarrm = {
    enable = true;
    package = jellyswarrmFixed;
    host = "127.0.0.1";
    port = 3001;
    dataDir = "${dataDirs.level5}/jellyswarrm";
    passwordFile = config.age.secrets.jellyswarrm-password.path;
  };

  services.nginx.virtualHosts."jellyswarrm.h.b.nel.family" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:3001";
      proxyWebsockets = true;
    };
  };
}
