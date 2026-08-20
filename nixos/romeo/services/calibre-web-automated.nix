{ config, ... }:
let
  dataDirs = config.data.dirs;
  domain = "books.nel.family";
  port = 8083;
in
{
  virtualisation.oci-containers.containers.calibre-web-automated = {
    # Pinned rather than :latest. CWA runs schema migrations on its app.db/cwa.db
    # at startup, and a major bump arriving unattended via podman-auto-update
    # would migrate the family's library DB with nobody watching. The autoupdate
    # label below still tracks this tag's digest, so point fixes land.
    image = "crocodilestick/calibre-web-automated:v4.0.6";

    # Deliberately no `user =`. This is a linuxserver.io s6-overlay image: the
    # entrypoint must start as root and drops to PUID/PGID itself. Setting
    # `user` here breaks the init sequence (cf. jellyfin.nix, which is a plain
    # image and *can* take one).
    environment = {
      "PUID" = "1000"; # bcnelson
      "PGID" = "100"; # users
      "TZ" = "America/Denver";
      # nginx is the only hop in front of the container, so CWA takes the real
      # client IP from the last X-Forwarded-For entry.
      "TRUSTED_PROXY_COUNT" = "1";
    };

    volumes = [
      # app.db + cwa.db (users, permissions, KOReader sync positions) and the
      # Calibre config. level3 so it is both snapshotted and sent offsite by
      # borg -- losing this loses every device's reading position.
      "${dataDirs.level3}/calibre/config:/config"
      # The Calibre library itself: metadata.db plus the book files. Purchased
      # and curated ebooks are not re-downloadable, so this is level3 rather
      # than sitting with the replaceable media tree on level6.
      "${dataDirs.level3}/calibre/library:/calibre-library"
      # Drop-zone. CWA *deletes* whatever it finds here once imported, so the
      # contents are inherently transient -> level6. Must not be nested inside
      # either bind above; CWA errors out on overlapping mounts.
      "${dataDirs.level6}/calibre-ingest:/cwa-book-ingest"
    ];

    ports = [ "127.0.0.1:${toString port}:8083" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  systemd.tmpfiles.rules = [
    "d ${dataDirs.level3}/calibre 0755 1000 100 - -"
    "d ${dataDirs.level3}/calibre/config 0755 1000 100 - -"
    # CWA creates metadata.db here on first start when the directory is empty.
    # It refuses to do so if the bind is owned root:root.
    "d ${dataDirs.level3}/calibre/library 0755 1000 100 - -"
    # Group-writable so books can be scp'd in as bcnelson without sudo. This
    # dir is not NFS-exported (romeo only exports /export/photos), so the other
    # route in is the web UI's upload button.
    "d ${dataDirs.level6}/calibre-ingest 0775 1000 100 - -"
  ];

  services.nginx = {
    enable = true;
    virtualHosts.${domain} = {
      forceSSL = true;
      enableACME = true;
      acmeRoot = null;
      extraConfig = ''
        # Ebook uploads through the web UI; comics/PDFs get large.
        client_max_body_size 0;
        #Allow access from Tailscale network
        allow 100.64.0.0/10;
        # books.nel.family CNAMEs to romeo.b.nel.family, which carries an AAAA
        # too, so a tailnet client can arrive over the ULA range.
        allow fd7a:115c:a1e0::/48;
        #Allow access from local network
        allow 192.168.0.0/16;
        deny all;
      '';
      # No auth_basic/auth_request anywhere on this vhost. /opds and /kosync do
      # their own HTTP Basic auth, and a second challenge in front of them
      # breaks KOReader outright -- it has no way to answer two of them.
      locations."/" = {
        proxyPass = "http://127.0.0.1:${toString port}";
        extraConfig = ''
          # CWA reads X-Scheme (cps/reverseproxy.py) when building absolute
          # URLs; without it the OPDS feed can advertise http:// links.
          proxy_set_header X-Scheme $scheme;
          # Importing or converting a large book can hold the request open well
          # past nginx's 60s default, and an e-reader on wifi downloads slowly.
          proxy_read_timeout 300s;
          proxy_send_timeout 300s;
        '';
      };
      # Book fetches are the one path that moves hundreds of MB to a device on
      # slow wifi. Stream them rather than letting nginx spool the whole file.
      locations."/opds/download/" = {
        proxyPass = "http://127.0.0.1:${toString port}";
        extraConfig = ''
          proxy_set_header X-Scheme $scheme;
          proxy_buffering off;
          proxy_read_timeout 600s;
          proxy_send_timeout 600s;
        '';
      };
    };
  };
}
