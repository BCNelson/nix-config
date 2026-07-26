{ pkgs, ... }: let
  # Waypoint authenticates by Tailscale identity. The password is a required
  # placeholder rather than a secret, so this can live in the store.
  connectionString =
    "mongodb://user:waypoint@waypoint-db.tailf3d5b.ts.net:27017,waypoint-db.tailf3d5b.ts.net:27018,waypoint-db.tailf3d5b.ts.net:27019/?replicaSet=Cluster0-shard-0&authSource=admin&tls=true";

  launcher = pkgs.writeShellScript "mongodb-redo-production-mcp-launcher" ''
    set -eu

    # Fail readably instead of hanging on a connection timeout.
    if command -v tailscale >/dev/null 2>&1 && ! tailscale status >/dev/null 2>&1; then
      echo "mongodb-redo-production MCP: tailnet is down; run 'tailscale up'" >&2
      exit 1
    fi

    MDB_MCP_CONNECTION_STRING="${connectionString}"
    export MDB_MCP_CONNECTION_STRING

    # npx resolves on its own (absolute store shebang), but the package bin it
    # execs carries `#!/usr/bin/env node`, and nodejs is on neither the login
    # PATH nor any agent's. Provide it here rather than inheriting it.
    export PATH="${pkgs.nodejs}/bin:$PATH"

    exec ${pkgs.nodejs}/bin/npx -y mongodb-mcp-server@latest "$@"
  '';
in {
  programs.mcp.servers.mongodb-redo-production = {
    command = "${launcher}";
    args = [ "--readOnly" ];
    startup_timeout_sec = 60;
  };
}
