{ pkgs, ... }: let
  # Upstream asks persistent registrations to pin one exact release rather than
  # track @latest; bump this by hand (`npm view rea-agents version`).
  version = "6.4.0";

  launcher = pkgs.writeShellScript "rea-mcp-launcher" ''
    set -eu

    # npx resolves on its own (absolute store shebang), but the package bin it
    # execs carries `#!/usr/bin/env node`, and nodejs is on neither the login
    # PATH nor any agent's. Provide it here rather than inheriting it.
    export PATH="${pkgs.nodejs}/bin:$PATH"

    # Native binary analysis. REA only connects to an existing Ghidra 12.1.x and
    # a full JDK (it compiles its bridge with javac); it installs neither.
    export GHIDRA_INSTALL_DIR="${pkgs.ghidra}/lib/ghidra"
    export JAVA_HOME="${pkgs.jdk21.home}"

    exec ${pkgs.nodejs}/bin/npx -y rea-agents@${version} mcp "$@"
  '';
in {
  programs.mcp.servers.rea = {
    command = "${launcher}";
    startup_timeout_sec = 30;
  };
}
