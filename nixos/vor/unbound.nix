{ pkgs, lib, ... }:
let
  zonefile = pkgs.writeTextFile {
    name = "dns-zonefile";
    text = ''
      photos.ck.nel.family A 192.168.138.40
    '';
    destination = "/localOverride";
  };

in
{
  services.unbound = {
    enable = true;
    settings = {
      server = {
        module-config = ''"respip validator iterator"'';
        interface = [ "0.0.0.0@53" "::@53" ];
        do-ip6 = "yes";
        access-control = [
          "127.0.0.1/32 allow"
          "192.168.0.0/16 allow"
          "10.0.0.0/8 allow"
          "fc00::/7 allow"
          "::1/128 allow"
        ];
        use-syslog = "yes";
        local-zone = [];
        local-data = [];
        serve-expired = "yes";
      };
      rpz = {
        name = "nel.family";
        zonefile = "${zonefile}/localOverride";
        rpz-log = "yes";
        rpz-log-name = "hairpin";
      };
    };
  };
  networking.networkmanager.dns = lib.mkForce "none";
  services.resolved.enable = lib.mkForce false;
  networking.firewall = {
    enable = true;
    allowedUDPPorts = [ 53 ];
  };
}
