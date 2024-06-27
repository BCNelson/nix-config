_:
let
  dataDirs = import ./dataDirs.nix;
in
{
  fileSystems."/export/photos" = {
    device = "${dataDirs.level2}/photos";
    options = [ "bind" "x-systemd.requires=zfs-import.target" ];
  };

  services.nfs = {
    enable = true;
    exports = ''
      /export/photos 100.64.0.0/10(rw,async)
    '';
  };
  networking.firewall.allowedTCPPorts = [ 2049 ];
}
