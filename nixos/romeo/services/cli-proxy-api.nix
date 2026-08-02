{ config, pkgs, ... }:
let
  port = 8317;
  stateDir = "/var/lib/cli-proxy-api";
in
{
  # Shared between the proxy (which accepts it) and LibreChat (which presents
  # it). Not a real OpenAI key -- it only authenticates localhost clients to the
  # proxy; the upstream credential is the OAuth token in stateDir.
  age.secrets.cli-proxy-api-key = {
    rekeyFile = ./secrets/cli_proxy_api_key.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };

  # The API key has to live inside the YAML, so the whole config is templated at
  # activation rather than generated into the world-readable store.
  age-template.files.cli-proxy-api-config = {
    vars = {
      API_KEY = config.age.secrets.cli-proxy-api-key.path;
    };
    owner = "cli-proxy-api";
    group = "cli-proxy-api";
    mode = "0400";
    content = ''
      # Upstream default is "" (all interfaces); LibreChat reaches it over loopback.
      host: "127.0.0.1"
      port: ${toString port}

      # OAuth token files, per-auth cooldown (.cds) state, and refreshed
      # credentials all land here, so it must stay writable.
      auth-dir: "${stateDir}"

      api-keys:
        - "$API_KEY"

      remote-management:
        allow-remote: false
        # Empty secret-key 404s every /v0/management route. disable-control-panel
        # additionally suppresses the management panel, which upstream otherwise
        # downloads from GitHub on first access and auto-updates periodically --
        # a runtime network fetch we do not want on a NixOS service.
        secret-key: ""
        disable-control-panel: true

      debug: false
      usage-statistics-enabled: false
    '';
  };

  # The OAuth bootstrap is a manual, non-declarative step: run the device-code
  # login once as the service user so the token lands in stateDir. It refreshes
  # itself from then on.
  #   sudo -u cli-proxy-api cli-proxy-api -config /run/agenix-template/cli-proxy-api-config -codex-device-login
  environment.systemPackages = [ pkgs.cli-proxy-api ];

  users.users.cli-proxy-api = {
    isSystemUser = true;
    group = "cli-proxy-api";
    home = stateDir;
  };
  users.groups.cli-proxy-api = { };

  systemd.services.cli-proxy-api = {
    description = "CLIProxyAPI - OpenAI-compatible proxy over CLI subscription credentials";
    wantedBy = [ "multi-user.target" ];
    wants = [ "network-online.target" ];
    after = [ "network-online.target" ];

    serviceConfig = {
      # Drop -local-model in ExecStart if the remote model catalog fetch at
      # startup ever becomes a problem; it falls back to the embedded catalog.
      ExecStart = "${pkgs.cli-proxy-api}/bin/cli-proxy-api -config ${config.age-template.files.cli-proxy-api-config.path}";
      User = "cli-proxy-api";
      Group = "cli-proxy-api";
      StateDirectory = "cli-proxy-api";
      StateDirectoryMode = "0700";
      WorkingDirectory = stateDir;
      Restart = "on-failure";
      RestartSec = "10s";

      NoNewPrivileges = true;
      PrivateTmp = true;
      PrivateDevices = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectControlGroups = true;
      RestrictNamespaces = true;
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      MemoryDenyWriteExecute = true;
      SystemCallArchitectures = "native";
      SystemCallFilter = [ "@system-service" "~@privileged" ];
      RestrictAddressFamilies = [ "AF_INET" "AF_INET6" "AF_UNIX" ];
    };
  };
}
