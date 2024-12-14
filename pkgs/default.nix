# Custom packages, that can be defined similarly to ones from nixpkgs
# You can build them using 'nix build .#example'
pkgs: {
  # example = pkgs.callPackage ./example { };
  mdns-reflector = pkgs.callPackage ./mdns-reflector.nix { };
  install-system = pkgs.callPackage ./install-system { };
  dolphin-shred = pkgs.callPackage ./dolphin-shred.nix { };
}
