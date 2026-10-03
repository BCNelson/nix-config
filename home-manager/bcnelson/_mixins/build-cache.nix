{ config, pkgs, ... }:

# Shared Rust/Go build caches plus a weekly sweeper, so per-project build
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
  home = config.home.homeDirectory;

  sweep = pkgs.writeShellApplication {
    name = "build-cache-sweep";
    runtimeInputs = with pkgs; [ coreutils findutils gnugrep go ];
    text = ''
      # DRY_RUN=1 build-cache-sweep  -> report what would be removed
      dry=''${DRY_RUN:-0}
      cargoBuildDays=14
      cargoTargetDays=30
      goDays=21
      goModcacheDays=30

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
    Unit.Description = "Prune idle Rust/Go build caches";
    Service = {
      Type = "oneshot";
      ExecStart = "${sweep}/bin/build-cache-sweep";
      Nice = 19;
      IOSchedulingClass = "idle";
    };
  };

  systemd.user.timers.build-cache-sweep = {
    Unit.Description = "Weekly Rust/Go build cache sweep";
    Timer = {
      OnCalendar = "weekly";
      Persistent = true;
      RandomizedDelaySec = "1h";
    };
    Install.WantedBy = [ "timers.target" ];
  };
}
