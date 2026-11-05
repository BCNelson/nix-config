{ pkgs, ... }:
let
  forgejoHost = "git.bcnelson.dev";
  forgejoUser = "bcnelson";

  # Declared by ../../../nixos/_mixins/roles/forgejo-agent-token.nix (and
  # system-manager/redo-3.nix). Hardcoded because home-manager cannot see the
  # system's config.age.secrets; this is agenix's default secretsDir.
  tokenFile = "/run/agenix/forgejo-agent-token";

  # forgejo-cli (`fj`) has no environment variable for a token: it only reads
  # $XDG_DATA_HOME/forgejo-cli/keys.json, written by `fj auth add-key`. Rather
  # than keep a second plaintext copy of the token in ~/.local/share, build a
  # keys.json per invocation in a private runtime dir -- the user's own
  # keys.json (logins to other instances) merged with the agenix token for
  # git.bcnelson.dev -- and point fj at it. Without the secret (host not yet
  # rekeyed) it is plain fj.
  #
  # FJ_FALLBACK_HOST makes `fj repo view owner/name` etc. target our instance
  # when run outside a checkout whose remote names a host.
  #
  # Side effect: `fj auth ...` changes made through this wrapper land in the
  # throwaway dir and are lost. Run the unwrapped binary for those
  # (`nix run nixpkgs#forgejo-cli -- auth ...`).
  fj = pkgs.writeShellApplication {
    name = "fj";
    runtimeInputs = [ pkgs.jq pkgs.coreutils ];
    text = ''
      real=${pkgs.forgejo-cli}/bin/fj
      export FJ_FALLBACK_HOST="''${FJ_FALLBACK_HOST:-https://${forgejoHost}}"
      if [ ! -r ${tokenFile} ]; then
        exec "$real" "$@"
      fi

      data_home="''${XDG_DATA_HOME:-$HOME/.local/share}"
      user_keys="$data_home/forgejo-cli/keys.json"
      tmp="$(mktemp -d "''${XDG_RUNTIME_DIR:-/tmp}/fj.XXXXXX")"
      trap 'rm -rf "$tmp"' EXIT
      mkdir -p "$tmp/forgejo-cli"
      if [ -r "$user_keys" ]; then base="$user_keys"; else base=/dev/null; fi
      (
        umask 077
        jq -n \
          --slurpfile base "$base" \
          --rawfile token ${tokenFile} \
          '($base[0] // {}) as $b
           | $b + {
               hosts: (($b.hosts // {}) + {"${forgejoHost}": {type: "Application", token: ($token | rtrimstr("\n"))}}),
               aliases: ($b.aliases // {}),
               default_ssh: ($b.default_ssh // [])
             }' \
          > "$tmp/forgejo-cli/keys.json"
      )
      # Not exec: the trap has to run to delete the token copy.
      XDG_DATA_HOME="$tmp" "$real" "$@"
    '';
  };

  # Lets plain git (and fj repo clone, which goes through libgit2's credential
  # helpers, and nix's git+https fetcher) read private repos over HTTPS.
  # Pushes still go over ssh (origin's pushurl), so the token being read-only
  # does not get in the way. Silent when the secret is missing, so git falls
  # through to its usual behaviour.
  credentialHelper = pkgs.writeShellScript "git-credential-forgejo-agent" ''
    [ "$1" = get ] || exit 0
    [ -r ${tokenFile} ] || exit 0
    echo "username=${forgejoUser}"
    printf 'password=%s\n' "$(cat ${tokenFile})"
  '';
in
{
  home.packages = [ fj ];

  programs.git = {
    enable = true;
    settings.credential."https://${forgejoHost}".helper = "${credentialHelper}";
  };
}
