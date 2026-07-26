{
  # Build the thin clients' system closures here and publish them to the
  # binary cache romeo already serves (see ./nixBinaryCacheProxy.nix). The
  # clients then update by downloading a finished closure — they never
  # evaluate or build anything themselves. See
  # services.bcnelson.remoteUpdate in nixos/delta/default.nix.
  services.bcnelson.closurePublisher = {
    enable = true;
    hosts = [ "delta-1" ];
    # Lets an installer booted on a thin client partition its disk, and be
    # installed at all, without evaluating this flake. See
    # nixos/delta/INSTALL.md.
    extraAttributes = [ "diskoScript" ];
    flakePath = "/config";
    # Same directory nginx serves for nixcache.nel.family.
    cacheDir = "/var/public-nix-cache";
    nginxVirtualHost = "nixcache.nel.family";

    # Publishing rides on autoUpdate rather than racing it: autoUpdate owns
    # /config, and it already runs both on its own timer and on a pushed ntfy
    # refresh, so chaining to it means every publish builds a checkout that
    # was just pulled. The timer below is only for the case where autoUpdate
    # somehow stops running at all.
    runAfter = [ "auto-update.service" ];
    interval = "6h";
  };
}
