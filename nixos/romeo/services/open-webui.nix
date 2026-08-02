{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  # Secret for JWT token generation
  age.secrets.open-webui-secret-key = {
    rekeyFile = ./secrets/open_webui_secret_key.age;
    generator.script = {pkgs, ...}: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };

  age-template.files.open-webui-env = {
    vars = {
      SECRET_KEY = config.age.secrets.open-webui-secret-key.path;
      # Authenticates open-webui to the local cli-proxy-api, which holds the
      # actual ChatGPT/Codex OAuth credential. See ./cli-proxy-api.nix.
      PROXY_API_KEY = config.age.secrets.cli-proxy-api-key.path;
    };
    content = ''
      WEBUI_SECRET_KEY=$SECRET_KEY
      OPENAI_API_KEY=$PROXY_API_KEY
    '';
  };

  services.open-webui = {
    enable = true;
    host = "127.0.0.1";
    port = 8085;
    stateDir = "${dataDirs.level5}/open-webui";
    environment = {
      OLLAMA_BASE_URL = "http://127.0.0.1:11434";
      DATA_DIR = "${dataDirs.level5}/open-webui";
      # NOTE: PersistentConfig var -- open-webui seeds it into its config table
      # on first launch and reads the DB thereafter, so changing it here has no
      # effect on an existing instance. Set it in Admin > Connections instead
      # (or set ENABLE_PERSISTENT_CONFIG=False to make this file authoritative).
      OPENAI_API_BASE_URL = "http://127.0.0.1:8317/v1";
    };
  };

  systemd.services.open-webui = {
    after = [ "zfs-import.target" "cli-proxy-api.service" ];
    requires = [ "zfs-import.target" ];
    wants = [ "cli-proxy-api.service" ];
    serviceConfig = {
      EnvironmentFile = config.age-template.files.open-webui-env.path;
      ReadWritePaths = [ "${dataDirs.level5}/open-webui" ];
    };
  };

  services.nginx.virtualHosts."ai.h.b.nel.family" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      #Allow access from Tailscale network
      allow 100.64.0.0/10;
      #Allow access from local network
      allow 192.168.0.0/16;
      deny all;
    '';
    locations = {
      "/" = {
        proxyPass = "http://127.0.0.1:8085";
        proxyWebsockets = true;
      };
    };
  };
}
