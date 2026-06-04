{ config, pkgs, ... }: let
  secretFile = "${config.home.homeDirectory}/.config/mcp-secrets/mongodb-redo-production";

  launcher = pkgs.writeShellScript "mongodb-redo-production-mcp-launcher" ''
    set -eu

    if [ ! -r "${secretFile}" ]; then
      echo "mongodb-redo-production MCP: missing ${secretFile}" >&2
      echo "Populate it with the MongoDB connection string (chmod 600)." >&2
      exit 1
    fi

    MDB_MCP_CONNECTION_STRING="$(${pkgs.coreutils}/bin/cat "${secretFile}")"
    export MDB_MCP_CONNECTION_STRING

    exec ${pkgs.nodejs}/bin/npx -y mongodb-mcp-server@latest "$@"
  '';
in {
  programs.mcp.servers.mongodb-redo-production = {
    command = launcher;
    args = [ "--readOnly" ];
    startup_timeout_sec = 60;
  };
}
