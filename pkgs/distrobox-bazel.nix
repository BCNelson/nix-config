{ pkgs }:

let
  # Wrapper scripts that run bazel/bazelisk inside the distrobox container
  # Uses explicit path to avoid calling the wrapper recursively
  # Adds /run/current-system/sw/bin to PATH to ensure docker is found
  bazelisk = pkgs.writeShellScriptBin "bazelisk" ''
    export PATH="/run/current-system/sw/bin:$PATH"
    exec ${pkgs.distrobox}/bin/distrobox enter redo -- /usr/local/bin/bazelisk "$@"
  '';

  bazel = pkgs.writeShellScriptBin "bazel" ''
    export PATH="/run/current-system/sw/bin:$PATH"
    exec ${pkgs.distrobox}/bin/distrobox enter redo -- /usr/local/bin/bazel "$@"
  '';

  distrobox-bazel-setup = pkgs.writeShellScriptBin "distrobox-bazel-setup" ''
    set -e
    echo "Setting up distrobox 'redo' container with bazelisk..."

    INI_FILE="''${XDG_CONFIG_HOME:-$HOME/.config}/distrobox/redo.ini"

    if [ ! -f "$INI_FILE" ]; then
      echo "Error: INI file not found at $INI_FILE"
      echo "Run 'home-manager switch' first to generate the config file."
      exit 1
    fi

    # Remove existing container if it exists
    if ${pkgs.distrobox}/bin/distrobox list | grep -q "^.*redo.*$"; then
      echo "Removing existing 'redo' container..."
      ${pkgs.distrobox}/bin/distrobox rm -f redo
    fi

    # Create container using assemble
    echo "Creating 'redo' container from $INI_FILE..."
    ${pkgs.distrobox}/bin/distrobox assemble create --file "$INI_FILE"

    echo "Done! You can now use 'bazel' and 'bazelisk' commands."
  '';
in
pkgs.symlinkJoin {
  name = "distrobox-bazel";
  paths = [ bazelisk bazel distrobox-bazel-setup ];
}
