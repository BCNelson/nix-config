{
  # Build the thin clients' system closures here and publish them to the
  # binary cache romeo already serves (see ./nixBinaryCacheProxy.nix). The
  # clients then update by downloading a finished closure — they never
  # evaluate or build anything themselves. See
  # services.bcnelson.remoteUpdate in nixos/delta/default.nix.
  services.bcnelson.closurePublisher = {
    enable = true;
    hosts = [ "delta-1" ];
    # Lets an installer booted on a thin client partition its disk without
    # evaluating this flake. See nixos/delta/INSTALL.md.
    extraAttributes = [ "diskoScript" ];
    flakePath = "/config";
    # Same directory nginx serves for nixcache.nel.family.
    cacheDir = "/var/public-nix-cache";
    nginxVirtualHost = "nixcache.nel.family";
    # Slightly ahead of the clients' 15m poll, so a change lands within about
    # half an hour of being pushed without keeping a builder busy constantly.
    interval = "30m";
  };
}
