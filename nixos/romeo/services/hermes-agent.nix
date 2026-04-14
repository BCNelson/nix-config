{ config, ... }:
let
  dataDirs = config.data.dirs;
in
{
  age.secrets.openrouter-api-key = {
    rekeyFile = ./secrets/openrouter_api_key.age;
  };

  age-template.files.hermes-agent-env = {
    vars = {
      OPENROUTER_API_KEY = config.age.secrets.openrouter-api-key.path;
    };
    content = ''
      OPENROUTER_API_KEY=$OPENROUTER_API_KEY
    '';
  };

  services.hermes-agent = {
    enable = true;
    stateDir = "${dataDirs.level5}/hermes-agent";
    addToSystemPackages = true;

    environmentFiles = [
      config.age-template.files.hermes-agent-env.path
    ];

    settings = {
      model = {
        default = "nousresearch/hermes-3-llama-3.1-405b";
        base_url = "https://openrouter.ai/api/v1";
      };
      smart_model_routing = {
        enabled = true;
        max_simple_chars = 160;
        max_simple_words = 28;
        cheap_model = {
          provider = "ollama";
          model = "qwen3:4b";
          base_url = "http://127.0.0.1:11434/v1";
        };
      };
      toolsets = [ "all" ];
      terminal = {
        backend = "local";
        timeout = 180;
      };
      memory = {
        memory_enabled = true;
        user_profile_enabled = true;
      };
    };
  };

  systemd.services.hermes-agent = {
    after = [ "zfs-import.target" "ollama.service" ];
    requires = [ "zfs-import.target" ];
    wants = [ "ollama.service" ];
  };
}
