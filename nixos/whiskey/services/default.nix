{ libx, dataDirs, ... }:
let
  vaultwarden = import ./defs/vaultwarden.nix { inherit dataDirs libx; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dockerComposeDefinition = {
      version = "3.8";
      services = builtins.foldl' (a: b: a // b) { } [
        vaultwarden
      ];
    };
  };
}
