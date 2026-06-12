{ config, inputs, ... }:
{
  imports = [ inputs.tendant.nixosModules.tendant ];

  # base64 AES-256 key for the credentials vault (TENDANT_CREDENTIALS_KEY).
  age.secrets.tendant-credentials-key = {
    rekeyFile = ./secrets/tendant_credentials_key.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -base64 32";
    owner = config.services.tendant.user;
    group = config.services.tendant.group;
    mode = "0440";
  };

  # Static, reusable device-pairing password (TENDANT_PASSWORD). Presented to
  # the pairDevice mutation; each device is then issued its own session token.
  # Retrieve the value to pair a device with: `agenix decrypt secrets/store/... `
  # (or read /run/agenix/tendant-password on the host).
  age.secrets.tendant-password = {
    rekeyFile = ./secrets/tendant_password.age;
    # Human-typeable passphrase (xkcdpass, 6 words) — entered during pairing.
    generator.script = "passphrase";
    owner = config.services.tendant.user;
    group = config.services.tendant.group;
    mode = "0440";
    bitwarden = {
      name = "Tendant Device Pairing";
      username = "owner";
      uris = {
        uri = "https://tendant.nel.family";
        matchType = "host";
      };
      notes = "Static device-pairing password for tendant on romeo-2. Enter it as the password in the pairDevice flow (displayName is any label for the device); each device then gets its own revocable session token.";
    };
  };

  services.tendant = {
    enable = false;
    httpAddr = "127.0.0.1:8095";
    # Provisions a local PostgreSQL + pgvector and a tendant DB/role; DSN
    # defaults to the local socket.
    enableLocalPostgres = true;
    credentials = {
      TENDANT_CREDENTIALS_KEY = config.age.secrets.tendant-credentials-key.path;
      TENDANT_PASSWORD = config.age.secrets.tendant-password.path;
    };
    # Rendered to /etc/tendant/tendant.toml. The overseer references a named
    # llm_connections entry; here we point it at the host's local ollama via
    # its OpenAI-compatible endpoint (no api_key needed — ollama ignores it,
    # and the client appends "/v1/chat/completions" to base_url itself).
    settings = {
      overseer.connection = "local-ollama";
      llm_connections = [
        {
          name = "local-ollama";
          provider = "openai";
          base_url = "http://127.0.0.1:11434";
          model = "qwen3:4b";
        }
      ];
    };
  };

  # The overseer talks to the local ollama daemon; start tendant after it.
  systemd.services.tendant = {
    wants = [ "ollama.service" ];
    after = [ "ollama.service" ];
  };

  services.nginx = {
    enable = true;
    virtualHosts."tendant.nel.family" = {
      forceSSL = true;
      enableACME = true;
      acmeRoot = null;
      http2 = true;
      locations."/" = {
        proxyPass = "http://127.0.0.1:8095";
        proxyWebsockets = true;
      };
    };
  };
}
