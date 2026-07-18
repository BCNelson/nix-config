# Custom packages, that can be defined similarly to ones from nixpkgs
# You can build them using 'nix build .#example'
pkgs: {
  # example = pkgs.callPackage ./example { };
  fork-init = pkgs.callPackage ./fork-init { };
  mdns-reflector = pkgs.callPackage ./mdns-reflector.nix { };
  install-system = pkgs.callPackage ./install-system { };
  dolphin-shred = pkgs.callPackage ./dolphin-shred.nix { };
  mb4-extractor = pkgs.callPackage ./m4b-extractor { };
  amazing-marvin = pkgs.callPackage ./amazing-marvin.nix { };
  opendeck = pkgs.callPackage ./opendeck.nix { };
  openhuman = pkgs.callPackage ./openhuman.nix { };
  age-bitwarden-sync = pkgs.callPackage ./age-bitwarden-sync { };
  codex-config-merge = pkgs.callPackage ./codex-config-merge { };
  ssh-mcp = pkgs.callPackage ./ssh-mcp { };
  happy-auth-notify = pkgs.callPackage ./happy-auth-notify { };
  kwin-adaptive-workspaces = pkgs.callPackage ./kwin-adaptive-workspaces { };
  spec-kit = pkgs.callPackage ./spec-kit.nix { };
  pince = pkgs.callPackage ./pince { };
  distrobox-bazel = pkgs.callPackage ./distrobox-bazel.nix { };
  nix-store-selinux = pkgs.callPackage ./nix-store-selinux.nix { };
  robocode = pkgs.callPackage ./robocode.nix { };
}
