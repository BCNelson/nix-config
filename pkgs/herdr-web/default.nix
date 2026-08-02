{ lib
, fetchFromGitHub
, buildNpmPackage
, rustPlatform
, importNpmLock
, makeWrapper
}:

let
  version = "0.4.0";

  src = fetchFromGitHub {
    owner = "kcosr";
    repo = "herdr-web";
    tag = "v${version}";
    hash = "sha256-vodfcykTrwK4iuZ1A1L5TMhdrn1cxJWTIoiQFOt9IbI=";
  };

  # The Vite app under web/ is self-contained (its tsconfig has no project
  # references and vite.config.ts reaches nowhere outside the directory), so
  # build it from that subtree rather than the whole checkout.
  #
  # importNpmLock rewrites package-lock.json so every dependency resolves to a
  # store path fetched from the integrity hashes already in the lock. That means
  # no npmDepsHash to regenerate on each bump - only the src hash above moves.
  # Note this is importNpmLock itself, not importNpmLock.buildNodeModules: the
  # latter hands back the *original* lock, so npmConfigHook would try to reach
  # registry.npmjs.org and fail the build with ENOTCACHED. buildNodeModules is
  # the devShell helper, and pairs with linkNodeModulesHook instead.
  webApp = buildNpmPackage {
    pname = "herdr-web-app";
    inherit version;
    src = "${src}/web";

    npmDeps = importNpmLock { npmRoot = "${src}/web"; };
    npmConfigHook = importNpmLock.npmConfigHook;

    # buildPhase is the default `npm run build` (tsc -b && vite build); only the
    # install needs overriding, because what we want is Vite's dist/ rather than
    # the packed node package npmInstallHook would produce.
    installPhase = ''
      runHook preInstall
      cp -r dist "$out"
      runHook postInstall
    '';
  };
in
rustPlatform.buildRustPackage {
  pname = "herdr-web";
  inherit version src;

  # bridge/ carries its own Cargo.lock and depends on ../vendor/herdr-compat by
  # relative path, so the whole checkout has to be the src while cargo runs one
  # level down. cargoRoot points the lockfile check at bridge/Cargo.lock;
  # buildAndTestSubdir is what actually moves cargo into bridge/.
  cargoRoot = "bridge";
  buildAndTestSubdir = "bridge";
  cargoLock.lockFile = "${src}/bridge/Cargo.lock";

  nativeBuildInputs = [ makeWrapper ];

  # Same layout upstream's scripts/package-tarball.sh produces. The bridge
  # defaults --static-dir to a relative "web/dist", which resolves against the
  # daemon's cwd and so is useless from a unit file; `herdr-web` is the wrapper
  # that pins it to the store copy, exactly like the released tarball's
  # bin/herdr-web.
  postInstall = ''
    mkdir -p "$out/share/herdr-web"
    cp -r ${webApp} "$out/share/herdr-web/web"

    makeWrapper "$out/bin/herdr-web-bridge" "$out/bin/herdr-web" \
      --add-flags "--static-dir $out/share/herdr-web/web"
  '';

  meta = {
    description = "Browser UI for herdr workspaces and agent panes";
    longDescription = ''
      A third-party HTTP/WebSocket bridge and React client for herdr. It is not
      associated with the upstream herdr project and vendors herdr's private
      protocol types, so it tracks one herdr version at a time - v0.4.0 needs
      herdr 0.7.5 or newer speaking terminal protocol 17. Check the release
      notes against pkgs.herdr before bumping.

      The bridge has no authentication of its own: its Host and Origin checks
      are a DNS-rebinding guard, nothing more. Bind it to loopback and put an
      authenticated path such as Tailscale Serve in front of it.
    '';
    homepage = "https://github.com/kcosr/herdr-web";
    license = lib.licenses.mit;
    mainProgram = "herdr-web";
    platforms = lib.platforms.unix;
  };
}
