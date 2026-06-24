{pkgs, ...}: let
  awsSupportLauncher = pkgs.writeShellScript "aws-support-mcp-server-launcher" ''
    set -eu

    latest_bin="$(${pkgs.findutils}/bin/find "$HOME/.cache/uv/archive-v0" -path '*/bin/awslabs.aws-support-mcp-server' -printf '%T@ %p\n' 2>/dev/null | ${pkgs.coreutils}/bin/sort -n | ${pkgs.coreutils}/bin/tail -n 1 | ${pkgs.gawk}/bin/awk '{ $1=""; sub(/^ /, ""); print }')"

    if [ -z "''${latest_bin:-}" ] || [ ! -x "$latest_bin" ]; then
      echo "aws-support-mcp-server binary not found in ~/.cache/uv/archive-v0" >&2
      exit 1
    fi

    exec "$latest_bin" "$@"
  '';
in {
  programs.mcp.servers.aws-support-mcp-server = {
    command = "${awsSupportLauncher}";
    startup_timeout_sec = 90;
    env = {
      AWS_PROFILE = "default";
      AWS_REGION = "us-east-1";
      FASTMCP_LOG_LEVEL = "ERROR";
    };
  };
}
