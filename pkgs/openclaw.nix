{
  lib,
  stdenvNoCC,
  buildPackages,
  fetchFromGitHub,
  fetchurl,
  fetchPnpmDeps,
  pnpmConfigHook,
  pnpm_11,
  nodejs-slim_22,
  makeWrapper,
  versionCheckHook,
  rolldown,
  installShellFiles,
}:
# Derived from nixpkgs' pkgs/by-name/op/openclaw/package.nix, carried here
# because nixpkgs tracks the 2026.6 maintenance line (14 versions, newest
# 2026.6.33) while upstream's Latest is 2026.7.1-2.
#
# The bump is not cosmetic. openclaw/openclaw#97933 fixes `/pair qr` rendering
# "⚠️ Media failed" in the Control UI: the webchat fallback handed the reply
# pipeline a `data:image/png` URL, which assertMediaNotDataUrl rejects outright.
# That merged 2026-06-29 and was never backported -- the merge commit compares
# as "diverged" from both v2026.6.33 and the later v2026.6.34, and "ahead" only
# from v2026.7.1-2 on. No nixpkgs revision carries it.
#
# Bumping this: change version, set each hash to lib.fakeHash in turn and read
# the real value out of the build error (src first; pnpmDeps only surfaces once
# src is correct). Also skim upstream's release notes for config-schema changes
# -- those do not fail the build, they fail the gateway at startup, so validate
# against nixos/romeo/services/openclaw.nix before deploying.
let
  pnpm = pnpm_11.override {nodejs-slim = nodejs-slim_22;};

  # Deliberately not "v${version}". Upstream re-tags a release with a -N
  # suffix (v2026.7.1-1, v2026.7.1-2 are the same release, both 2026-08-04)
  # while the binary keeps reporting plain "2026.7.1". versionCheckHook greps
  # its output for finalAttrs.version, so folding the suffix into version
  # fails the build with the misleading
  #   Did not find version 2026.7.1-2 in the output of ... --version
  # followed by a --help retry, which prints a wall of usage text and no error.
  releaseTag = "v2026.7.1-2";

  # The bundled Matrix channel plugin (extensions/matrix) depends on
  # @matrix-org/matrix-sdk-crypto-nodejs, whose npm tarball ships no binary at
  # all: a postinstall script downloads the prebuilt napi module from GitHub
  # releases. pnpmConfigHook installs with lifecycle scripts disabled, so the
  # package lands in the output with index.js and no matrix-sdk-crypto.*.node
  # beside it.
  #
  # That is a *runtime* failure, not a build one, and it is narrow: E2EE text
  # goes through matrix-js-sdk's initRustCrypto, which uses the pure-wasm
  # @matrix-org/matrix-sdk-crypto-wasm and does ship complete. Only
  # encryptMedia/decryptMedia in extensions/matrix/src/matrix/sdk/crypto-facade.ts
  # touch the native binding -- i.e. images and voice notes in encrypted rooms,
  # which includes the `/pair qr` reply this version exists to fix.
  #
  # Left missing, the failure mode is worse than a plain error:
  # ensureMatrixCryptoRuntime() catches the missing-binding require() and
  # spawns `node download-lib.js` with cwd set to the package directory, which
  # is a read-only store path. So it fails with an EROFS-flavoured download
  # error after a network round trip, on every encrypted media send.
  #
  # Vendoring the binding makes that first require() succeed, so the downloader
  # branch is never reached.
  #
  # No patchelf needed: the module needs libgcc_s.so.1, libm and libc, and node
  # has all three loaded via its own RPATH before it dlopens this, so the
  # soname lookup resolves against already-loaded objects. Verified by
  # process.dlopen()ing this exact file under nodejs-slim_22.
  #
  # Bumping openclaw: check whether pnpm still resolves
  # @matrix-org/matrix-sdk-crypto-nodejs to 0.6.1 (the version is in
  # extensions/matrix/package.json, and download-lib.js builds its URL from
  # that same version). A stale binding here does not fail the build -- it
  # fails media sends at runtime, so the version has to be checked by hand.
  #
  # Only x86_64-linux is vendored, because romeo is the only consumer.
  # Elsewhere the file is simply absent and encrypted media falls back to the
  # broken downloader path; add an entry to fix that, using the asset names
  # from download-lib.js.
  matrixCryptoNativeVersion = "0.6.1";
  matrixCryptoNativeBySystem = {
    x86_64-linux = rec {
      name = "matrix-sdk-crypto.linux-x64-gnu.node";
      src = fetchurl {
        url = "https://github.com/matrix-org/matrix-rust-sdk-crypto-nodejs/releases/download/v${matrixCryptoNativeVersion}/${name}";
        hash = "sha256-yw0qhr1nIfgtmIrmsMVMw8XTZP9ZoG+Vk061h9w1yfQ=";
      };
    };
  };
  matrixCryptoNative =
    matrixCryptoNativeBySystem.${stdenvNoCC.hostPlatform.system} or null;
in
  stdenvNoCC.mkDerivation (finalAttrs: {
    pname = "openclaw";
    version = "2026.7.1";

    src = fetchFromGitHub {
      owner = "openclaw";
      repo = "openclaw";
      tag = releaseTag;
      hash = "sha256-kpiKCTjXX4l525IJDNsnI7j2IT6ZYdqvFTyRlKGgomg=";
    };

    pnpmDepsHash = "sha256-/ou2Hoix9m/be6kq4Osg4gTTQQRTkL5uLOuERmevuQ0=";

    pnpmDeps = fetchPnpmDeps {
      inherit (finalAttrs) pname version src;
      inherit pnpm;
      fetcherVersion = 4;
      hash = finalAttrs.pnpmDepsHash;
    };

    # The Dream Diary subagent turn is capped by a hardcoded constant, not by
    # config or an env var -- upstream's only knob is editing this line. 60s was
    # chosen against hosted APIs; it is too tight for the local ollama path on
    # romeo, where the diary model is usually cold at 03:00 and has to be paged
    # off disk into the B580 before it emits a token.
    #
    # Measured on romeo with the real narrative prompt (gemma4:12b, num_ctx
    # 8192): 6.5s warm, 20-26s cold, and 46s cold with thinking enabled. The
    # default path is the fast one -- the effective agent thinking level is
    # "off" (embedded-agent-runner/compact.ts), which the ollama plugin forwards
    # as think=false -- so 20-26s is the number to plan around. But that is a
    # measurement on an otherwise idle GPU, and the sweep runs at 03:00 against
    # the same B580 that serves interactive turns; a contended cold load can
    # still cross a minute.
    #
    # Do NOT try to pin this with `params.think = false` on the model entry.
    # extensions/ollama/src/stream.ts reads model.params.think only as a guard
    # and never forwards it: setting it makes the runtime's own think=false stop
    # being sent, so ollama falls back to the model default, which is thinking
    # ON. It does the opposite of what it reads like.
    #
    # Blowing the timeout is not a hard failure, which is why it is easy to miss:
    # appendFallbackNarrativeEntry writes a synthetic placeholder entry into
    # DREAMS.md instead ("A memory trace surfaced, but details were
    # unavailable in this run."), so the diary silently degrades to filler.
    #
    # 5 minutes is the cap because the sweep is a detached nightly cron job with
    # nothing waiting on it -- there is no user-facing latency to protect, only
    # the risk of a stalled subagent holding the job open, which the original
    # comment on this constant is about. Three phases each waiting the full
    # worst case is still bounded at 15 minutes on an idle box.
    #
    # Bumping openclaw: --replace-fail means a reworded or retuned upstream
    # constant fails the build loudly rather than silently reverting to 60s.
    postPatch = ''
      substituteInPlace extensions/memory-core/src/dreaming-narrative.ts \
        --replace-fail 'const NARRATIVE_TIMEOUT_MS = 60_000;' \
                       'const NARRATIVE_TIMEOUT_MS = 300_000;'
    '';

    buildInputs = [rolldown];

    nativeBuildInputs = [
      pnpmConfigHook
      pnpm
      nodejs-slim_22
      makeWrapper
      installShellFiles
    ];

    buildPhase = ''
      runHook preBuild

      pnpm install --frozen-lockfile

      # Replace pnpm-installed rolldown with the Nix-built version
      rm -rf node_modules/rolldown node_modules/@rolldown/pluginutils
      mkdir -p node_modules/@rolldown node_modules/.pnpm/node_modules/@rolldown
      cp -r ${rolldown}/lib/node_modules/rolldown node_modules/rolldown
      cp -r ${rolldown}/lib/node_modules/@rolldown/pluginutils node_modules/@rolldown/pluginutils
      cp -r ${rolldown}/lib/node_modules/rolldown node_modules/.pnpm/node_modules/rolldown
      cp -r ${rolldown}/lib/node_modules/@rolldown/pluginutils node_modules/.pnpm/node_modules/@rolldown/pluginutils
      chmod -R u+w node_modules/rolldown node_modules/@rolldown/pluginutils \
        node_modules/.pnpm/node_modules/rolldown node_modules/.pnpm/node_modules/@rolldown/pluginutils

      pnpm build
      pnpm ui:build

      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall

      libdir=$out/lib/openclaw
      mkdir -p $libdir $out/bin


      cp --reflink=auto -r package.json dist node_modules $libdir/
      cp --reflink=auto -r docs skills patches extensions qa $libdir/

      # Not in the nixpkgs recipe, which was written against 2026.6.x. 2026.7.x
      # moved runtime code into pnpm workspace packages under packages/*, which
      # node_modules only ever symlinks (relatively:
      # node_modules/@openclaw/ai -> ../../packages/ai). Copying node_modules
      # without packages/ leaves those links dangling and the CLI dies with
      #   Cannot find package '@openclaw/ai' imported from .../dist/errors-*.js
      # which fails installShellCompletion below and versionCheckHook after it.
      # 4.7M for 21 packages.
      cp --reflink=auto -r packages $libdir/

      # packages/*/node_modules/.bin carries pnpm workspace links back to the
      # repo root (e.g. speech-core/node_modules/.bin/openclaw ->
      # node_modules/openclaw/openclaw.mjs), which do not exist in the output.
      # noBrokenSymlinks fails the build on those, so sweep them the same way
      # the recipe already sweeps extensions/ below.
      find $libdir/packages -xtype l -delete
      ${lib.optionalString (matrixCryptoNative != null) ''

        # Native crypto binding for the Matrix channel plugin; see the long note
        # in the let block above for why it is fetched rather than installed.
        # Placed in the hoisted copy of the package because that is where
        # createRequire() from dist/extensions/matrix resolves it, and index.js
        # then loads the .node relative to itself. matrix-sdk-crypto-nodejs is
        # hoisted to node_modules/@matrix-org as a real directory (no .pnpm
        # indirection), so there is exactly one place to put this.
        install -Dm444 ${matrixCryptoNative.src} \
          $libdir/node_modules/@matrix-org/matrix-sdk-crypto-nodejs/${matrixCryptoNative.name}
      ''}

      mkdir -p $libdir/src
      cp --reflink=auto -r src/agents $libdir/src/

      rm -f $libdir/node_modules/.pnpm/node_modules/clawdbot \
        $libdir/node_modules/.pnpm/node_modules/moltbot \
        $libdir/node_modules/.pnpm/node_modules/openclaw-control-ui

      # Remove broken symlinks created by pnpm workspace linking in extensions
      find $libdir/extensions -xtype l -delete
      # Remove symlinks pointing back to the build sandbox
      find $libdir/dist/extensions -type l -lname "$NIX_BUILD_TOP/*" -delete

      makeWrapper ${lib.getExe nodejs-slim_22} $out/bin/openclaw \
        --add-flags "$libdir/dist/index.js" \
        --set NODE_PATH "$libdir/node_modules"
      ln -s $out/bin/openclaw $out/bin/moltbot
      ln -s $out/bin/openclaw $out/bin/clawdbot

      runHook postInstall
    '';

    postInstall = lib.optionalString (stdenvNoCC.hostPlatform.emulatorAvailable buildPackages) (
      let
        emulator = stdenvNoCC.hostPlatform.emulator buildPackages;
      in ''
        installShellCompletion --cmd openclaw \
          --bash <(${emulator} $out/bin/openclaw completion --shell bash) \
          --fish <(${emulator} $out/bin/openclaw completion --shell fish) \
          --zsh  <(${emulator} $out/bin/openclaw completion --shell zsh)
      ''
    );

    nativeInstallCheckInputs = [versionCheckHook];
    doInstallCheck = true;

    meta = {
      description = "Self-hosted, open-source AI assistant/agent";
      longDescription = ''
        Self-hosted AI assistant/agent connected to all your apps on your Linux
        or macOS machine and controlled via your choice of chat app.

        Note: Project is in early/rapid development and uses LLMs to parse untrusted
        content while having full access to system by default.

        Parsing untrusted input with LLMs leaves them vulnerable to prompt injection.

        (Originally known as Moltbot and ClawdBot)
      '';
      homepage = "https://openclaw.ai";
      changelog = "https://github.com/openclaw/openclaw/releases/tag/${releaseTag}";
      license = lib.licenses.mit;
      mainProgram = "openclaw";
      platforms = with lib.platforms; linux ++ darwin;
      # Deliberately NOT carrying nixpkgs' knownVulnerabilities entry.
      #
      # The risk it names is real and unchanged -- this parses untrusted content
      # with an LLM while holding full system access, so prompt injection is a
      # design property rather than a fixable CVE. It is dropped only because
      # the marker is an eval-time gate, and keeping it here would mean
      # re-adding a permittedInsecurePackages entry that has to be edited by
      # hand on every version bump. On this host the mitigation is the systemd
      # sandbox in nixos/romeo/services/openclaw.nix, which is what actually
      # bounds a misbehaving agent; read that before widening its scope.
      #
      # Side effect worth knowing: the marker is also why nixpkgs' build is
      # `available = false`, so Hydra never builds openclaw and no binary cache
      # has it. Every host that wants this compiles it (~15 min, 3.1G output).
    };
  })
