{ pkgs, ... }:

# Testcontainers' own reaper (Ryuk) doesn't run under rootless Podman, so
# containers from interrupted or crashed test runs are never removed. They
# pile up: 65 CockroachDB/Redis containers once held ~37 GiB of RAM plus most
# of zram. Anything Testcontainers started that is older than any real test
# run gets removed.
#
# Uses the distro's /usr/bin/podman rather than nixpkgs' so it talks to the
# same storage and version the tests do (this is for Fedora hosts).
let
  reap = pkgs.writeShellApplication {
    name = "testcontainers-reap";
    runtimeInputs = [ pkgs.coreutils ];
    text = ''
      # DRY_RUN=1 testcontainers-reap  -> list what would be removed
      maxAge=''${MAX_AGE:-2h}
      podman=/usr/bin/podman

      # Different filter keys are ANDed; `until` matches on creation time.
      mapfile -t ids < <(
        "$podman" ps -a -q \
          --filter label=org.testcontainers.managed-by=testcontainers \
          --filter "until=$maxAge"
      )
      if [ "''${#ids[@]}" -eq 0 ]; then
        echo "no testcontainers older than $maxAge"
        exit 0
      fi

      "$podman" ps -a --format '{{.ID}} {{.Image}} {{.RunningFor}} {{.Names}}' \
        --filter label=org.testcontainers.managed-by=testcontainers \
        --filter "until=$maxAge"
      if [ "''${DRY_RUN:-0}" = 0 ]; then
        # Test databases don't need a graceful shutdown.
        "$podman" rm -f -t 5 "''${ids[@]}" >/dev/null
        echo "removed ''${#ids[@]} testcontainers older than $maxAge"
      fi
    '';
  };
in
{
  home.packages = [ reap ];

  systemd.user.services.testcontainers-reap = {
    Unit.Description = "Remove Testcontainers containers older than 2 hours";
    Service = {
      Type = "oneshot";
      ExecStart = "${reap}/bin/testcontainers-reap";
    };
  };

  systemd.user.timers.testcontainers-reap = {
    Unit.Description = "Reap stale Testcontainers every 15 minutes";
    Timer = {
      OnBootSec = "15min";
      OnUnitActiveSec = "15min";
    };
    Install.WantedBy = [ "timers.target" ];
  };
}
