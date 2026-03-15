{ libx, dataDirs, pkgs, ... }:
let
  nextcloud = import ./defs/nextcloud.nix { inherit dataDirs libx; };
  vikunja = import ./defs/vikunja.nix { inherit dataDirs libx pkgs; };
  paperless = import ./defs/paperless.nix { inherit dataDirs libx; };
  tubearchivist = import ./defs/tubearchivist.nix { inherit dataDirs libx; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dockerComposeDefinition = {
      services = builtins.foldl' (a: b: a // b) { } [
        nextcloud
        vikunja
        paperless
        tubearchivist
      ];
    };
  };
}
