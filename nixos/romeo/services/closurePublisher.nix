{ config, ... }:
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
    # Backstop only — the ntfy subscription below is what makes a push land
    # promptly. ntfy_refresh_topic is declared in ../default.nix.
    interval = "30m";
    ntfy-refresh = {
      enable = true;
      topicFile = config.age.secrets.ntfy_refresh_topic.path;
      # /config is autoUpdate's checkout, and both subscribers wake on the
      # same message. Let the pull finish before building from it.
      afterUnits = [ "auto-update.service" ];
    };
  };
}
