{ lib
, stdenvNoCC
, fetchFromGitHub
, bun
, nodejs
, herdr
}:

# Browser and phone client for herdr -- https://github.com/devswha/herdr-web-ui
#
# Upstream expects `herdr plugin install devswha/herdr-web-ui`, which
# git-clones into herdr's managed checkout and runs `bun install` and
# `bun run build` there. Neither fits the store, so this does both here and
# lays out the plugin directory that install would have produced, which the
# herdr mixin registers in plugins.json the same way it does herdr-mirror.
stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "herdr-web-ui";
  version = "0.4.12";

  src = fetchFromGitHub {
    owner = "devswha";
    repo = "herdr-web-ui";
    tag = "v${finalAttrs.version}";
    hash = "sha256-TwmeSDag7iSGGmR1aTylNY4y3Dbfbk0FYr/dxMFl89A=";
  };

  # bun has no lockfile fetcher in nixpkgs, so the dependency tree is a
  # fixed-output derivation, the same shape nixpkgs uses for opencode. Every
  # os/cpu variant is installed so the hash is the same on every platform;
  # the extra node-pty prebuilds are small. --ignore-scripts because nothing
  # here needs an install script: node-pty ships prebuilt binaries as
  # optional dependencies, and bun applies patchedDependencies itself.
  #
  # Two trees, one hash: `full` builds the client, and `production` is what
  # ships. The full tree is ~1.2G of typescript and rolldown binaries for
  # every platform that the server never loads.
  node_modules = stdenvNoCC.mkDerivation {
    pname = "${finalAttrs.pname}-node_modules";
    inherit (finalAttrs) version src;

    impureEnvVars = lib.fetchers.proxyImpureEnvVars ++ [ "GIT_PROXY_COMMAND" "SOCKS_SERVER" ];
    nativeBuildInputs = [ bun ];
    dontConfigure = true;

    buildPhase = ''
      runHook preBuild
      export HOME=$TMPDIR
      flags="--frozen-lockfile --ignore-scripts --no-progress --os=* --cpu=*"
      bun install $flags
      mv node_modules full
      bun install $flags --production
      mv node_modules production
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      mkdir "$out"
      mv full production "$out"
      runHook postInstall
    '';

    # A fixed-output derivation must not reference the store.
    dontFixup = true;

    outputHash = "sha256-EokXJFXcc6NFWuR86Ka5yQHtBCrbzYg8YG+QD03pSdo=";
    outputHashAlgo = "sha256";
    outputHashMode = "recursive";
  };

  nativeBuildInputs = [ bun nodejs ];

  configurePhase = ''
    runHook preConfigure
    cp -R ${finalAttrs.node_modules}/full node_modules
    chmod -R u+w node_modules
    # `bun run build` spawns node_modules/.bin/vite, whose `#!/usr/bin/env
    # node` does not exist in the sandbox.
    patchShebangs node_modules
    runHook postConfigure
  '';

  buildPhase = ''
    runHook preBuild
    export HOME=$TMPDIR
    bun run build
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    root="$out/${finalAttrs.passthru.pluginRoot}"
    mkdir -p "$root"
    rm -rf node_modules
    cp -R . "$root"
    cp -R ${finalAttrs.node_modules}/production "$root/node_modules"
    chmod -R u+w "$root/node_modules"

    # Every plugin command goes through with-bun.sh, which only looks for bun
    # and node in the usual home-directory install places. Put ours first; the
    # server also spawns `node` for each terminal and `herdr` for its CLI.
    substituteInPlace "$root/scripts/with-bun.sh" \
      --replace-fail '. "$(dirname "$0")/bun-path.sh"' \
        'PATH=${lib.makeBinPath [ bun nodejs herdr ]}''${PATH:+:$PATH}; export PATH'
    runHook postInstall
  '';

  # The prebuilt binaries are other platforms' or already stripped.
  dontStrip = true;

  # The plugin root herdr should be pointed at; see the herdr web-ui mixin.
  passthru.pluginRoot = "share/herdr/plugins/herdr-web-ui";

  meta = {
    description = "Browser and phone client for herdr";
    homepage = "https://github.com/devswha/herdr-web-ui";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
})
