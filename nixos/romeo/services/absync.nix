{
  config,
  inputs,
  ...
}:
# ABSync — syncs audiobooks from AudioBookshelf to USB MP3 players and Yoto
# cards. The web server runs here at absync.h.b.nel.family; the service and its
# hardening come from the ABSync flake's own module.
#
# The server has no authentication of its own, so the vhost carries two gates:
#  - Tailnet/LAN only. absync.h.b has an explicit CNAME to romeo.b (romeo's
#    Tailscale address, main.tf) so it beats the public *.h.b wildcard, and
#    tailnet clients arrive with their 100.64/10 or ULA source. LAN clients
#    never see that record -- romeo's unbound answers *.h.b with 192.168.3.7 --
#    so the LAN is allowed too. The public h.b ingress preserves client
#    addresses, so internet traffic is refused here.
#  - authentik SSO via forward auth (../authentik-forward-auth.nix), which
#    admits service_admins.
let
  dataDirs = config.data.dirs;
  host = "absync.h.b.nel.family";
  # The module default (3000) is everybody's default; pin something unused.
  port = 8098;

  dataDir = "${dataDirs.level4}/absync";
in {
  imports = [inputs.absync.nixosModules.default];

  services.absync-server = {
    enable = true;
    host = "127.0.0.1";
    inherit port;
    # state.redb (sync state). On the vault so snapshots + borg pick it up.
    inherit dataDir;
    # Cover cache: rebuildable.
    cacheDir = "${dataDirs.level7}/absync";
  };

  # dataDir is on a ZFS-native mount, which systemd only tracks passively, so
  # RequiresMountsFor alone can race it at boot and write to the empty root-disk
  # directory underneath. Order against zfs-mount.service explicitly.
  systemd.services.absync-server = {
    requires = ["zfs-mount.service"];
    after = ["zfs-mount.service"];
    unitConfig.RequiresMountsFor = [dataDir];
  };

  services.nginx.virtualHosts.${host} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      client_max_body_size 0;

      allow 100.64.0.0/10;
      allow fd7a:115c:a1e0::/48;
      allow 192.168.0.0/16;
      deny all;
    '';
    locations = import ../authentik-forward-auth.nix {
      "/" = {
        proxyPass = "http://127.0.0.1:${toString port}";
        proxyWebsockets = true;
      };
    };
  };
}
