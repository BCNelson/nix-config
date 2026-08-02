{ pkgs, ... }:
let
  domain = "homefirst.dev";
  # Canonical home of the monorepo (Forgejo on this same host). CI's pages.yml
  # force-pushes the built Astro/Starlight tree to `pages` as a single orphan
  # commit; nothing in CI reaches into whiskey.
  repoUrl = "https://git.bcnelson.dev/home-first/homefirst.git";
  branch = "pages";

  stateDir = "/var/lib/homefirst-pages";
  checkout = "${stateDir}/checkout"; # the git working tree (branch `pages`)
  releases = "${stateDir}/releases"; # one .git-free copy per deployed commit
  current = "${stateDir}/current"; # symlink -> releases/<sha>; nginx's root
in
{
  # Replaces Codeberg Pages, which served this site until the repo moved off
  # Codeberg. Forgejo has no Pages service, so whiskey serves the static tree
  # itself and PULLS it — the alternative (CI pushing over ssh/rsync) would mean
  # handing CI a shell credential on this host, where a read-only git fetch does.
  #
  # The `pages` branch must be readable anonymously. If the repo is ever made
  # private, add an agenix token secret and switch repoUrl to embed it.

  services.nginx = {
    enable = true;
    virtualHosts."${domain}" = {
      forceSSL = true;
      enableACME = true; # DNS-01 via the role default (porkbun)
      acmeRoot = null;
      serverAliases = [ "www.${domain}" ];
      root = current;
      locations."/" = {
        # Astro emits page.html files; serve them without the extension too.
        tryFiles = "$uri $uri.html $uri/index.html =404";
      };
      extraConfig = ''
        # `root` is a symlink that flips on each deploy, as are the files under
        # it; nginx must follow them.
        disable_symlinks off;
      '';
    };
  };

  # Pull the built site and publish it atomically. Each commit becomes its own
  # release directory and `current` is flipped with an atomic rename, so nginx
  # never serves a half-copied tree.
  systemd.services.homefirst-pages-sync = {
    description = "Sync the homefirst.dev static site from the repo's pages branch";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    path = [
      pkgs.git
      pkgs.coreutils
    ];
    serviceConfig = {
      Type = "oneshot";
      StateDirectory = "homefirst-pages";
      # nginx (running as its own user) has to read the published tree.
      StateDirectoryMode = "0755";
      UMask = "0022";
    };
    script = ''
      set -euo pipefail

      if [ -d ${checkout}/.git ]; then
        git -C ${checkout} fetch --depth 1 origin ${branch}
        git -C ${checkout} checkout -q --detach FETCH_HEAD
        git -C ${checkout} clean -qfdx
      else
        rm -rf ${checkout}
        git clone --depth 1 --single-branch --branch ${branch} ${repoUrl} ${checkout}
      fi

      sha="$(git -C ${checkout} rev-parse HEAD)"
      target="${releases}/$sha"

      if [ ! -d "$target" ]; then
        rm -rf "$target.partial"
        mkdir -p "$target.partial"
        cp -a ${checkout}/. "$target.partial/"
        rm -rf "$target.partial/.git"
        mv -T "$target.partial" "$target"
      fi

      # Atomic publish: create the new symlink under a temp name, then rename
      # it over `current` (rename(2) on a symlink is atomic).
      ln -sfn "$target" ${current}.new
      mv -Tf ${current}.new ${current}
      echo "published $sha"

      # Keep the three most recent releases plus whatever `current` points at.
      # Note the careful shell here: this runs under `set -e`, so a bare
      # `[ ... ] && continue` would abort the script on the first release that
      # isn't current, and an unguarded `ls` would abort when releases/ is empty.
      keep="$(readlink -f ${current} || true)"
      old_releases="$(ls -1dt ${releases}/* 2>/dev/null | tail -n +4 || true)"
      for old in $old_releases; do
        if [ "$(readlink -f "$old")" != "$keep" ]; then
          rm -rf "$old"
        fi
      done
    '';
  };

  systemd.timers.homefirst-pages-sync = {
    description = "Periodically sync the homefirst.dev static site";
    wantedBy = [ "timers.target" ];
    timerConfig = {
      # Populate the docroot soon after boot, then poll for new deploys.
      OnBootSec = "30s";
      OnUnitActiveSec = "5min";
    };
  };
}
