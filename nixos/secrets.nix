{ hostname, pkgs, lib, ... }:
let
  hosts = import ./hosts.nix;
in
{
  services.openssh = {
    enable = lib.mkDefault true;
  };

  age.rekey = {
    storageMode = "local";
    localStorageDir = ../secrets/hosts/${hostname};
    hostPubkey = hosts.${hostname};
    masterIdentities = [
      {
        identity = ../secrets/masterKeys/yubikey5cblack.pub;
        pubkey = "age1yubikey1qgw5sthxazuy96nq4cnldd7wydn4jf59cc5sc5fglmjnh2getqu4g2rmyfj";
      }
      {
        identity = ../secrets/masterKeys/soloback.hmac;
        pubkey = "age1qknx4qlm8qse85afs5np42kf2rsh28j9jvyzdd3n7gljpclhep9qrt2qrt";
      }
    ];
    agePlugins = [ pkgs.age-plugin-fido2-hmac ];
  };
}
