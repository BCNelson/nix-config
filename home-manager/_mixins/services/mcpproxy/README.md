# Local MCP gateway

MCPProxy runs as a systemd user service on `127.0.0.1:8640`. The pinned
upstream binary includes its web UI. There is no external database, token
provisioner, or renewal daemon. MCPProxy manages upstream OAuth and refresh
tokens in its private state directory (`~/.local/state/mcpproxy`).

Local MCP connections are trusted and do not require authentication. Profile
URLs select subsets of servers; they are not authorization boundaries. Local
clients can select another profile or use the unfiltered endpoint. The REST
API/web UI still use MCPProxy's automatically generated admin key.

## Configuration

The workstation MCP mixin enables the service, moves remote upstream definitions
to `services.mcpproxy.upstreams`, and connects Codex, Claude Code, OpenCode and Pi.
Local browser, Kubernetes, AWS and database tools retain their existing setup.

```nix
services.mcpproxy.upstreams.notion.url = "https://mcp.notion.com/mcp";
services.mcpproxy.profiles.codex = lib.mkForce [ "notion" ];
services.mcpproxy.settings.features.enable_web_ui = false;
```

Profile endpoints are `/mcp/p/codex`, `/mcp/p/claude`, `/mcp/p/opencode`,
`/mcp/p/pi`, and `/mcp/p/shared`. The shared profile is available through the
generic Home Manager MCP configuration for other clients. MCPProxy profile
endpoints expose `retrieve_tools` and `call_tool_*`: clients discover upstream
tools through search and then invoke them through the gateway.

Nix renders configuration on service startup. The small shell/jq startup step
preserves the generated admin key and OAuth registration fields for unchanged
upstreams; Nix settings win on conflicts. Upstream tokens remain in the embedded
store. UI edits to server definitions and policy last until the next restart;
make lasting changes in Nix. Do not put secrets in Nix settings.

## First use

Apply the Home Manager configuration using the normal host deployment workflow,
then check:

```sh
systemctl --user status mcpproxy
mcpproxy status
mcpproxy auth login --all
```

The packaged `mcpproxy` wrapper selects the configured state and config paths.
Open `http://127.0.0.1:8640/ui/` for the UI. `mcpproxy status` reports connection
information, including the admin key; keep its output private.

Complete each provider's browser consent once in MCPProxy. Existing Codex/Claude
OAuth sessions are not imported. Restart/reconnect agent MCP sessions after
activation. Authentication to real providers must be checked interactively.

The OAuth callback is a separate loopback listener. On a remote machine, arrange
SSH forwarding for its callback port or perform the initial login locally.
Back up the private state directory with the service stopped to preserve OAuth
registrations and tokens. It contains secrets; it is not encrypted by this module.

## Credentials

MCPProxy can accept a pre-generated **admin** key via `MCPPROXY_API_KEY` (for
example from an agenix-backed systemd environment file). Its scoped agent-token
create/regenerate APIs generate the token themselves; the pinned version offers
no supported config or CLI import for pre-generated agent tokens. This local
setup needs neither kind of credential in agent configuration.
