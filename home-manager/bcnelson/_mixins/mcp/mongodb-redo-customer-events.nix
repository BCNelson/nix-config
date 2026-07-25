{ pkgs, ... }: let
  # Separate Waypoint listener for the customer-events cluster. `customerevents`
  # exists on both clusters, so keep the two servers distinctly named.
  connectionString =
    "mongodb://user:waypoint@waypoint-db.tailf3d5b.ts.net:27020,waypoint-db.tailf3d5b.ts.net:27021,waypoint-db.tailf3d5b.ts.net:27022/?authSource=admin&tls=true";

  launcher = pkgs.writeShellScript "mongodb-redo-customer-events-mcp-launcher" ''
    set -eu

    # Fail readably instead of hanging on a connection timeout.
    if command -v tailscale >/dev/null 2>&1 && ! tailscale status >/dev/null 2>&1; then
      echo "mongodb-redo-customer-events MCP: tailnet is down; run 'tailscale up'" >&2
      exit 1
    fi

    MDB_MCP_CONNECTION_STRING="${connectionString}"
    export MDB_MCP_CONNECTION_STRING

    exec ${pkgs.nodejs}/bin/npx -y mongodb-mcp-server@latest "$@"
  '';
in {
  programs.mcp.servers.mongodb-redo-customer-events = {
    command = "${launcher}";
    args = [ "--readOnly" ];
    startup_timeout_sec = 60;
  };
}
