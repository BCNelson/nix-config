# Custom packages, that can be defined similarly to ones from nixpkgs
# You can build them using 'nix build .#example'
pkgs: {
  # example = pkgs.callPackage ./example { };
  mdns-reflector = pkgs.callPackage ./mdns-reflector.nix { };
  install-system = pkgs.callPackage ./install-system { };
  dolphin-shred = pkgs.callPackage ./dolphin-shred.nix { };
  mb4-extractor = pkgs.callPackage ./m4b-extractor { };
  claude-code = pkgs.callPackage ./claude-code { };
  amazing-marvin = pkgs.callPackage ./amazing-marvin.nix { };
  age-bitwarden-sync = pkgs.callPackage ./age-bitwarden-sync { };
}
