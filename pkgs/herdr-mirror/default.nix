{ lib
, rustPlatform
, fetchFromGitHub
, makeWrapper
, herdr
, openssh
}:

# A herdr plugin that mirrors a remote herdr server's workspaces and agents into
# the local sidebar -- https://github.com/nikok6/herdr-mirror
#
# Upstream expects `herdr plugin install nikok6/herdr-mirror`, which git-clones
# into herdr's managed checkout and runs scripts/install.sh to download a
# release binary. Neither half survives here, so this builds the crate and lays
# out the plugin directory that installer would have produced (see postInstall).
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "herdr-mirror";
  version = "0.2.1";

  src = fetchFromGitHub {
    owner = "nikok6";
    repo = "herdr-mirror";
    tag = "v${finalAttrs.version}";
    hash = "sha256-8zOPdMrR/FsSWwWoaLLobwwSg+ylZbzqPTOWWMD5820=";
  };

  cargoHash = "sha256-6zH1+E0WL8wPKZgV60CvlULC1A24HSu25eRge+yleBI=";

  nativeBuildInputs = [ makeWrapper ];

  postInstall = ''
    # herdr runs a plugin's action commands from its plugin root, and the
    # manifest names ./target/release/herdr-mirror -- the path scripts/install.sh
    # would have fetched the release binary into. Recreating that layout keeps
    # the manifest usable verbatim, so an upstream bump never needs it rewritten.
    mkdir -p "$out/share/herdr/plugins/mirror/target/release"
    cp herdr-plugin.toml "$out/share/herdr/plugins/mirror/herdr-plugin.toml"
    ln -s "$out/bin/herdr-mirror" "$out/share/herdr/plugins/mirror/target/release/herdr-mirror"

    # The daemon shells out to both: ssh for every remote, and `herdr status
    # --json` to find the local socket when HERDR_SOCKET_PATH is not already in
    # the environment (i.e. every invocation that did not come from herdr).
    #
    # A suffix, not a prefix: whatever ssh the host already ships must keep
    # winning. On a non-NixOS host it is the one built against that distro's
    # /etc/ssh/ssh_config -- Fedora's includes crypto-policies options that this
    # openssh rejects outright ("Bad configuration option: gssapikexalgorithms",
    # then terminate), so preferring ours breaks every remote on redo-3. This is
    # only here so the daemon still has an ssh when PATH carries none.
    wrapProgram "$out/bin/herdr-mirror" \
      --suffix PATH : ${lib.makeBinPath [ herdr openssh ]}
  '';

  # The plugin root herdr should be pointed at; see the herdr-mirror mixin.
  passthru.pluginRoot = "share/herdr/plugins/mirror";

  meta = {
    description = "Mirror a remote herdr server's workspaces and agents into the local sidebar";
    homepage = "https://github.com/nikok6/herdr-mirror";
    license = lib.licenses.mit;
    mainProgram = "herdr-mirror";
    platforms = lib.platforms.unix;
  };
})
