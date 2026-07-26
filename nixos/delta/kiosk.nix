# What the thin client actually shows: cage + firefox on a fixed set of URLs,
# the same services.bcnelson.sign module charlie uses for the kitchen display.
#
# Kept in its own file so swapping the workload (a remote desktop client, a
# local session, nothing at all) is a one-line change in ./default.nix rather
# than surgery on the host config.
{
  services.bcnelson.sign = {
    enable = true;
    # TODO: point at whatever this particular unit should display.
    urls = [ "https://homeassistant.h.b.nel.family/" ];
  };

  programs.firefox.preferences = {
    # One content process instead of the default eight. Each one costs
    # 80–150 MiB, which is real money against 2 GiB; a kiosk showing a
    # single origin gains nothing from the isolation.
    "dom.ipc.processCount" = 1;
    "fission.autostart" = false;
    # Cap the disk cache explicitly (KiB) — the default sizes itself from
    # free space, and there is very little of that on an 8 GiB eMMC.
    "browser.cache.disk.capacity" = 65536;
    "browser.cache.disk.smart_size.enabled" = false;
    # Session restore writes to flash every 15s by default. This host has
    # nothing worth restoring.
    "browser.sessionstore.interval" = 600000;
    # No update machinery: the system closure pins the browser version, and
    # the updater cannot write to the read-only store anyway.
    "app.update.auto" = false;
    "extensions.update.enabled" = false;
  };
}
