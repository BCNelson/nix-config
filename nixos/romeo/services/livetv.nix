# Live TV for Jellyfin from streaming subscriptions (ESPN Unlimited first).
#
# FastChannels gathers channels and guide data from the providers it is signed
# in to and serves them as M3U + XMLTV. ESPN's streams are Widevine DRM, which
# no IPTV client can decrypt, so FastChannels hands those channels to PrismCast:
# a real Google Chrome on a virtual display plays FastChannels' own /watch page,
# ffmpeg captures the screen, and PrismCast serves the result as a plain stream.
# Quality is therefore a re-encode of what Chrome played (720p60 by default;
# ESPN's linear networks are produced at 720p60 anyway), stereo only.
#
# Jellyfin's Live TV setup (done once, in its dashboard):
#   tuner (M3U):  http://fastchannels:5523/m3u/prismcast/ts
#   guide (XMLTV): http://fastchannels:5523/epg.xml
# The /ts variant gives Jellyfin MPEG-TS rather than PrismCast's fMP4 HLS.
{ config, pkgs, ... }:
let
  dataDirs = config.data.dirs;

  # Shared with jellyfin so it reaches FastChannels -- and PrismCast, which
  # lives in the same network namespace -- by the name "fastchannels".
  network = "livetv";
  networkEnsure = [ "-${pkgs.podman}/bin/podman network create ${network}" ];

  fcPort = 5523;
  pcPort = 5589;
  novncPort = 6080;

  # Tailnet only: neither UI has a login by default, FastChannels holds every
  # provider's tokens, and noVNC is remote control of a Chrome signed in to
  # them. *.romeo.b.nel.family is a wildcard to romeo's Tailscale address
  # (main.tf), and the h.b ingress preserves the client address, so anything
  # from outside Tailscale's ranges is refused.
  mkVhost = port: {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      allow 100.64.0.0/10;
      allow fd7a:115c:a1e0::/48;
      deny all;
    '';
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString port}";
      proxyWebsockets = true;
      extraConfig = "proxy_read_timeout 1h;";
    };
  };
in
{
  virtualisation.oci-containers.containers.fastchannels = {
    image = "ghcr.io/kineticman/fastchannels:latest";
    # PrismCast joins this container's network namespace, so its ports are
    # published here.
    ports = [
      "127.0.0.1:${toString fcPort}:5523"
      "127.0.0.1:${toString pcPort}:5589"
      "127.0.0.1:${toString novncPort}:6080"
    ];
    volumes = [
      # SQLite DB with every provider sign-in: level3, backed up offsite.
      "${dataDirs.level3}/fastchannels:/data"
    ];
    environment = {
      # Base for the stream URLs in the playlists; it is jellyfin that fetches
      # them, over the shared network.
      PUBLIC_BASE_URL = "http://fastchannels:${toString fcPort}";
      PRISMCAST_URL = "http://fastchannels:${toString pcPort}";
      # Where PrismCast's Chrome loads the /watch pages. Widevine only runs in a
      # secure context, and plain http to a container name is not one; loopback
      # is, and the two containers share a loopback.
      PRISMCAST_INNER_URL = "http://127.0.0.1:${toString fcPort}";
    };
    extraOptions = [ "--network=${network}" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  virtualisation.oci-containers.containers.prismcast = {
    image = "ghcr.io/hjdhjd/prismcast:latest";
    dependsOn = [ "fastchannels" ];
    volumes = [
      # Config, channels and the Chrome profile holding provider sessions.
      "${dataDirs.level3}/prismcast:/root/.prismcast"
    ];
    environment = {
      QUALITY_PRESET = "720p-high";
      # Each stream is a Chrome tab (~1-2 GB) plus an encode; romeo has the
      # headroom for a few, not the default 10.
      MAX_CONCURRENT_STREAMS = "4";
      # Plex-only HDHomeRun emulation; jellyfin uses the M3U.
      HDHR_ENABLED = "false";
    };
    extraOptions = [
      "--network=container:fastchannels"
      "--shm-size=2g"
      # The entrypoint only looks at renderD128 (for Xvfb's DRI3 backing and
      # VA-API encode), so hand it the A380 -- the media GPU jellyfin and
      # frigate use -- under that name rather than whichever card the kernel
      # numbered first. The by-driver links are 0666 (2.hardware-configuration.nix).
      "--device=/dev/dri/by-driver/i915-render:/dev/dri/renderD128"
      "--device=/dev/dri/by-driver/i915-card:/dev/dri/card0"
    ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };

  # Keep jellyfin on podman's default network too, so nothing about how it is
  # published or reaches the internet changes; the second network only adds a
  # route (and aardvark DNS) to fastchannels.
  virtualisation.oci-containers.containers.jellyfin.extraOptions = [
    "--network=podman"
    "--network=${network}"
  ];

  systemd.services.podman-fastchannels.serviceConfig.ExecStartPre = networkEnsure;
  systemd.services.podman-jellyfin.serviceConfig.ExecStartPre = networkEnsure;

  # PrismCast lives in fastchannels' network namespace, which dies with that
  # container. dependsOn only orders the start, so also stop/restart PrismCast
  # with it (BindsTo/PartOf) and bring it back whenever fastchannels comes up
  # again (wantedBy), e.g. after podman auto-update recreates it.
  systemd.services.podman-prismcast = {
    bindsTo = [ "podman-fastchannels.service" ];
    partOf = [ "podman-fastchannels.service" ];
    wantedBy = [ "podman-fastchannels.service" ];
  };

  systemd.tmpfiles.rules = [
    "d ${dataDirs.level3}/fastchannels 0700 root root - -"
    "d ${dataDirs.level3}/prismcast 0700 root root - -"
  ];

  services.nginx.virtualHosts = {
    "fastchannels.romeo.b.nel.family" = mkVhost fcPort;
    "prismcast.romeo.b.nel.family" = mkVhost pcPort;
    "prismcast-vnc.romeo.b.nel.family" = mkVhost novncPort;
  };
}
