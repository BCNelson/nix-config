{ config, lib, ... }:
let
  cfg = config.services.mcpproxy;
  profileUrl = name: "http://127.0.0.1:${toString cfg.port}/mcp/p/${name}";
  agentProfiles = [ "shared" "codex" "claude" "opencode" "pi" ];
in
{
  imports = [
    ../../../_mixins/services/mcpproxy
    ../../../_mixins/services/config-merge.nix
  ];

  config = lib.mkMerge [
    {
      programs.mcp.enable = true;
      services.mcpproxy.enable = lib.mkDefault true;
    }
    (lib.mkIf cfg.enable {
      services.mcpproxy.settings = {
        enable_code_execution = true;
        routing_mode = "code_execution";
      };

      # All agents initially see every configured upstream. A host can replace
      # an individual profile with a normal assignment (no mkForce needed).
      services.mcpproxy.profiles = lib.genAttrs agentProfiles (
        _: lib.mkDefault (builtins.attrNames cfg.upstreams)
      );

      programs.mcp.servers.gateway.url = profileUrl "shared";
      programs.claude-code.mcpServers.gateway = lib.mkIf config.programs.claude-code.enable {
        type = "http";
        url = profileUrl "claude";
      };
      programs.opencode.settings.mcp.gateway = lib.mkIf config.programs.opencode.enable {
        type = "remote";
        url = profileUrl "opencode";
      };
      # Codex's writable configuration is owned by config-merge in ../codex.
      services.config-merge.codex.settings.mcp_servers.gateway.url =
        lib.mkIf config.programs.codex.enable (lib.mkForce (profileUrl "codex"));

      home.file.".pi/agent/mcp.json".text = builtins.toJSON {
        mcpServers.gateway.url = profileUrl "pi";
      };
    })
  ];
}
