{ libx, dataDirs, pkgs, ... }:
let
  porkbun_api_creds = libx.getSecretWithDefault ../sensitive.nix "porkbun_api" {
    api_key = "";
    secret_key = "";
  };
  porkbun = pkgs.writeTextFile {
    name = "porkbun-dns-config";
    text = ''
      dns_porkbun_key=${porkbun_api_creds.api_key}
      dns_porkbun_secret=${porkbun_api_creds.secret_key}
    '';
    destination = "/porkbun.ini";
  };
  swag = import ./defs/swag.nix { inherit dataDirs porkbun; };
  jellyfin = import ./defs/jellyfin.nix { inherit dataDirs; };
  audiobooks = import ./defs/audiobooks.nix { inherit dataDirs; };
  nextcloud = import ./defs/nextcloud.nix { inherit dataDirs libx; };
  vikunja = import ./defs/vikunja.nix { inherit dataDirs libx; };
  mealie = import ./defs/mealie.nix { inherit dataDirs libx; };
  syncthing = import ./defs/syncthing.nix { inherit dataDirs; };
  foundryvtt = import ./defs/foundryvtt.nix { inherit dataDirs libx; };
  fastenhealth = import ./defs/fastenhealth.nix { inherit dataDirs; };
  homebox = import ./defs/homebox.nix { inherit dataDirs; };
  immich = import ./defs/immich.nix { inherit dataDirs libx; };
  dashy = import ./defs/dashy { inherit pkgs; };
  authelia = import ./defs/authelia { inherit dataDirs libx pkgs; };
  paperless = import ./defs/paperless.nix { inherit dataDirs libx; };
  tubearchivist = import ./defs/tubearchivist.nix { inherit dataDirs libx; };
in
{
  networkBacked = libx.createDockerComposeStackPackage {
    name = "general";
    src = ./defs/config;
    dependencies = [ porkbun ];
    dockerComposeDefinition = {
      services = builtins.foldl' (a: b: a // b) { } [
        swag
        jellyfin
        audiobooks
        nextcloud
        vikunja
        mealie
        syncthing
        foundryvtt
        fastenhealth
        homebox
        immich
        dashy
        authelia
        paperless
        tubearchivist
      ];
    };
  };
}
