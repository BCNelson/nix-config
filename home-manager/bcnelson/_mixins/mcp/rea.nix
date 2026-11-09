{ pkgs, ... }: let
  # Upstream asks persistent registrations to pin one exact release rather than
  # track @latest; bump this by hand (`npm view rea-agents version`).
  version = "6.8.0";

  # REA's Ghidra bridge only admits one release line (GhidraReleasePolicy.js:
  # SUPPORTED_GHIDRA_VERSION, currently 12.1.4, accepting any 12.1.x) and
  # refuses everything else, so nixpkgs' ghidra (12.2+) leaves it with no
  # decompiler. ghidra-bin just unpacks the upstream release zip, so pin that
  # to the verified build. When bumping `version`, check the bundled
  # SUPPORTED_GHIDRA_VERSION and move this with it.
  ghidra = pkgs.ghidra-bin.overrideAttrs (_: {
    version = "12.1.4";
    versiondate = "20260921";
    src = pkgs.fetchzip {
      url = "https://github.com/NationalSecurityAgency/ghidra/releases/download/Ghidra_12.1.4_build/ghidra_12.1.4_PUBLIC_20260921.zip";
      hash = "sha256-ZD4Vs1i7BMvrneAbAiOc7F9iBj64Rbom+PaVO//TzK8=";
    };
  });

  launcher = pkgs.writeShellScript "rea-mcp-launcher" ''
    set -eu

    # npx resolves on its own (absolute store shebang), but the package bin it
    # execs carries `#!/usr/bin/env node`, and nodejs is on neither the login
    # PATH nor any agent's. Provide it here rather than inheriting it.
    export PATH="${pkgs.nodejs}/bin:$PATH"

    # Native binary analysis. REA only connects to an existing Ghidra and a
    # full JDK (it compiles its bridge with javac); it installs neither. The
    # JDK must fall inside the bounds the Ghidra install declares in its
    # application.properties (12.1.x: JDK 21).
    export GHIDRA_INSTALL_DIR="${ghidra}/lib/ghidra"
    export JAVA_HOME="${pkgs.jdk21.home}"

    exec ${pkgs.nodejs}/bin/npx -y rea-agents@${version} mcp "$@"
  '';
in {
  programs.mcp.servers.rea = {
    command = "${launcher}";
    startup_timeout_sec = 30;
  };
}
