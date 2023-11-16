_:
let
  zonefile = pkgs.writeTextFile {
    name = "linode-dns-config";
    text = ''
      todo.nel.family A 192.168.3.5
      recipes.nel.family A 192.168.3.5
      h.b.nel.family A 192.168.3.5
      media.nel.family A 192.168.3.5
      notes.bnel.me A 192.168.3.5
      audiobooks.nel.family A 192.168.3.5
      changedetection.nel.family A 192.168.3.5
      nel.to A 192.168.3.5
    '';
    destination = "/localOverride";
  };

in
{
  services.unbound = {
    enable = true;
    settings = {
      interface = [ "0.0.0.0@53" "::@53" ];
      do-ipv6 = true;
      access-control = [
        "127.0.0.1/32 allow"
        "192.168.0.0/16 allow"
        "172.16.0.0.0/12 allow"
        "10.0.0.0/8 allow"
        "fc00::/7 allow"
        "::1/128 allow"
      ];
      rpz = {
        name = "nel.family";
        zonefile = "${zonefile}/localOverride";
        rpz-log = "yes";
        rpz-log-name = "hairpin";
      };
      forward-zone = {
        name = ".";
        forward-addr = [ "8.8.8.8" "8.8.4.4" ];
      };
      local-zone = [
        "atlas.h.b.nel.family redirect"
        "hypnos.h.b.nel.family redirect"
        "h.b.nel.family redirect"
      ];
      local-data = [
        "atlas.h.b.nel.family. IN A 192.168.3.5"
        "hypnos.h.b.nel.family. IN A 192.168.3.6"
        "h.b.nel.family. IN A 192.168.3.5"
        "notes.bnel.me. IN A 192.168.3.5"
      ];
    };
  };
}
