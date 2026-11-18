{ config, pkgs, ... }:

# Shared Rust/Go build caches plus a nightly sweeper, so per-project build
# output stops piling up on the root disk. Everything here works without
# touching any project's devenv.nix:
#
# - Cargo discovers ~/.cargo/config.toml by walking up from the project dir
#   (and devenv leaves CARGO_HOME alone), so every project under $HOME picks up
#   sccache and the shared build-dir.
# - Go projects whose devenv points GOCACHE/GOMODCACHE into .devenv/state are
#   handled by the sweeper deleting those caches once the project goes idle;
#   Go only self-trims a build cache when it runs against it, so abandoned
#   projects otherwise keep theirs forever.
let

  sweep = pkgs.writeShellApplication {
    name = "build-cache-sweep";
    runtimeInputs = with pkgs; [ coreutils findutils gnugrep gnused go ];
    text = ''
      # DRY_RUN=1 build-cache-sweep  -> report what would be removed
      dry=''${DRY_RUN:-0}
      cargoBuildDays=14
      cargoTargetDays=30
      goDays=21
      goModcacheDays=30
      direnvDays=14

      remove() {
        echo "remove $1 ($(du -sh "$1" | cut -f1)): $2"
        if [ "$dry" = 0 ]; then
          chmod -R u+w "$1" 2>/dev/null || true
          rm -rf "$1"
        fi
      }

      # True when anything under $1 was written in the last $2 days.
      recentlyUsed() {
        [ -n "$(find "$1" -newermt "-$2 days" -print -quit 2>/dev/null)" ]
      }

      # Cargo build-dir: ~/.cargo/build/<xx>/<workspace-hash>
      for d in "$HOME"/.cargo/build/*/*/; do
        [ -d "$d" ] || continue
        recentlyUsed "$d" "$cargoBuildDays" || remove "''${d%/}" "cargo build-dir idle ''${cargoBuildDays}d"
      done

      # Cargo target dirs (tagged by cargo's CACHEDIR.TAG), wherever they live
      mapfile -t targets < <(
        find "$HOME/dev" "$HOME/.cache" -maxdepth 8 \
          \( -name .git -o -name node_modules -o -name .devenv -o -name .direnv \) -prune \
          -o -name CACHEDIR.TAG -type f -print 2>/dev/null |
          while read -r tag; do
            grep -qs 'created by cargo' "$tag" && dirname "$tag"
          done | sort
      )
      # Cargo also tags per-triple dirs (target/wasm32-...); judge only the outermost.
      outer=""
      for d in "''${targets[@]}"; do
        if [ -n "$outer" ] && [[ "$d" == "$outer"/* ]]; then continue; fi
        outer=$d
        recentlyUsed "$d" "$cargoTargetDays" || remove "$d" "cargo target idle ''${cargoTargetDays}d"
      done

      # Per-project Go caches from devenv. Go rewrites trim.txt (at most daily)
      # whenever it uses a build cache, so its age is the project's idle time.
      mapfile -t goCaches < <(
        find "$HOME/dev" -maxdepth 8 \( -name .git -o -name node_modules \) -prune \
          -o -type d -path '*/.devenv/state/go-cache' -print 2>/dev/null
      )
      for d in "''${goCaches[@]}"; do
        if [ -n "$(find "$d/trim.txt" -newermt "-$goDays days" 2>/dev/null)" ]; then
          continue
        fi
        remove "$d" "go build cache idle ''${goDays}d"
        mod="$(dirname "$d")/go-mod-cache"
        [ -d "$mod" ] && remove "$mod" "go module cache of idle project"
      done

      # nix-direnv GC roots. Every project with `use flake` / `use devenv`
      # pins its dev shell plus each flake input source through
      # .direnv/flake-profile-* and .direnv/flake-inputs/*, so an abandoned
      # checkout keeps gigabytes of toolchains alive forever. Dropping the
      # roots lets the nightly nix-gc reclaim them; the next `cd` into the
      # project rebuilds (mostly substitutes) the shell. devenv itself
      # registers no persistent roots, so this covers it too.
      #
      # "Last used" is the newest mtime of the profile link and .rc file:
      # nix-direnv touches both on every load, even "Using cached dev shell"
      # with nothing changed (checked 2026-11-18 on use-flake devenv and
      # plain flake projects), so it is the last activation to the second.
      # The .rc atime is folded in as a fallback (/ is relatime); it can only
      # make a project look newer, never older. Never use the link's atime:
      # readlink bumps it, and nix-gc readlinks every root nightly.
      mapfile -t direnvDirs < <(
        for l in /nix/var/nix/gcroots/auto/*; do readlink "$l"; done 2>/dev/null |
          grep "^$HOME/.*/\.direnv/" | sed -E 's#/\.direnv/.*#/.direnv#' | sort -u
      )
      cutoff=$(date -d "-$direnvDays days" +%s)
      for d in "''${direnvDirs[@]}"; do
        [ -d "$d" ] || continue
        last=$(
          {
            stat -c %Y "$d"/flake-profile-* "$d"/nix-profile-*
            stat -c '%X %Y' "$d"/*.rc
          } 2>/dev/null | tr ' ' '\n' | sort -n | tail -1
        )
        [ "''${last:-0}" -ge "$cutoff" ] && continue
        echo "drop direnv GC roots in ''${d%/.direnv} (last used $(date -d "@''${last:-0}" +%F)): direnv idle ''${direnvDays}d"
        if [ "$dry" = 0 ]; then
          rm -rf "$d"/flake-profile-* "$d"/nix-profile-* "$d"/flake-inputs
        fi
      done

      # Global Go module cache: Go never trims it, so reset it periodically.
      stamp="''${XDG_STATE_HOME:-$HOME/.local/state}/build-cache-sweep/go-modcache"
      mkdir -p "$(dirname "$stamp")"
      if [ -z "$(find "$stamp" -newermt "-$goModcacheDays days" 2>/dev/null)" ]; then
        modcache="$(go env GOMODCACHE)"
        if [ -d "$modcache" ]; then
          echo "remove $modcache ($(du -sh "$modcache" | cut -f1)): monthly go module cache reset"
          [ "$dry" = 0 ] && go clean -modcache
        fi
        [ "$dry" = 0 ] && touch "$stamp"
      fi
      echo "done"
    '';
  };
in
{
  home.packages = [ pkgs.sccache sweep ];

  # build-dir moves cargo's intermediate artifacts out of each project's
  # target/ (which then holds only final binaries) into one place the sweeper
  # can age out per workspace.
  home.file.".cargo/config.toml".text = ''
    [build]
    rustc-wrapper = "${pkgs.sccache}/bin/sccache"
    build-dir = "{cargo-cache-home}/build/{workspace-path-hash}"
  '';

  # sccache evicts least-recently-used entries past this size on its own.
  xdg.configFile."sccache/config".text = ''
    [cache.disk]
    dir = "${config.xdg.cacheHome}/sccache"
    size = ${toString (20 * 1024 * 1024 * 1024)}
  '';

  systemd.user.services.build-cache-sweep = {
    Unit.Description = "Prune idle Rust/Go build caches and direnv GC roots";
    Service = {
      Type = "oneshot";
      ExecStart = "${sweep}/bin/build-cache-sweep";
      Nice = 19;
      IOSchedulingClass = "idle";
    };
  };

  systemd.user.timers.build-cache-sweep = {
    Unit.Description = "Nightly Rust/Go build cache sweep";
    Timer = {
      # Nightly rather than weekly: a single week of active Rust work can put
      # ~100G in ~/.cargo/build, and on a weekly cadence an idle workspace
      # could linger up to 21 days instead of 14. The sweep itself is cheap
      # (~1 min, idle IO class) when there is little to remove. 23:00 so the
      # system nix-gc at 00:00 reclaims dropped direnv roots the same night.
      OnCalendar = "*-*-* 23:00:00";
      Persistent = true;
      RandomizedDelaySec = "1h";
    };
    Install.WantedBy = [ "timers.target" ];
  };
}
