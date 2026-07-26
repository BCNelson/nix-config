{
  # Build the limited hosts' system closures here and publish them to the
  # binary cache romeo already serves (see ./nixBinaryCacheProxy.nix). Those
  # hosts never evaluate or build anything themselves — not when updating
  # (services.bcnelson.remoteUpdate) and not when first installed
  # (`install-system --limited`), which waits on the manifests below.
  services.bcnelson.closurePublisher = {
    enable = true;

    # `install-system --limited` adds new hosts here, at the marker, in the
    # same pull request that adds them to the flake. A host missing from this
    # list is a host nobody is building.
    hosts = [
      # INSERT_NEW_LIMITED_HOST_HERE
    ];

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
