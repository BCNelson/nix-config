{ libx, dataDirs, pkgs, ... }:
let
  dns_linode_key = libx.getSecret ../../sensitive.nix "dns_linode_key";
  porkbun_api_key = libx.getSecret ../sensitive.nix "porkbun_api_key";
  porkbun_api_secret = libx.getSecret ../sensitive.nix "porkbun_api_secret";
  porkbunToken = pkgs.writeTextFile {
    name = "porkbun-dns-config";
    text = ''
      dns_porkbun_key=${porkbun_api_key}
      dns_porkbun_secret=${porkbun_api_secret}
    '';
    destination = "/porkbun.ini";
  };
  swag = import ./defs/swag.nix { inherit dataDirs porkbunToken; };
  vaultwarden = import ./defs/vaultwarden.nix { inherit dataDirs libx; };
  healthchecks = import ./defs/healthchecks.nix { inherit dataDirs libx; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dependencies = [ porkbunToken ];
    dockerComposeDefinition = {
      version = "3.8";
      services = builtins.foldl' (a: b: a // b) { } [
        swag
        vaultwarden
        healthchecks
      ];
    };
  };
}
