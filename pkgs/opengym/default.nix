{
  lib,
  applyPatches,
  buildNpmPackage,
  fetchFromGitHub,
  fetchFromGitLab,
  makeWrapper,
  nodejs_24,
}:
# openGym — self-hosted gym & body-weight tracker. Upstream ships only a docker
# compose stack; this is the same three pieces built natively:
#
#   opengym-api    the Node backend (passkeys, per-user JSON state, web push)
#   opengym-web    the React SPA, prebuilt to static files for nginx to serve
#   opengym-media  the exercise images/GIFs the SPA loads from /img/ and /gif/
#
# Consumed by nixos/romeo/services/opengym.nix, which is where the nginx vhost
# that stitches them into one origin lives — the API and the SPA must share an
# origin or WebAuthn refuses to run at all.
let
  version = "1.2.9";

  src = applyPatches {
    name = "opengym-${version}-source";

    src = fetchFromGitLab {
      owner = "DuarteSantos8";
      repo = "opengym";
      tag = "v${version}";
      hash = "sha256-HU/GeUh7KBUcEup232RaqqSuaNyvAIz8LvO7tQrBlMg=";
    };

    # Adds OpenID Connect (authorization-code + PKCE) beside the existing passkey
    # login, wired to the same signed session cookie, plus the "Sign in with …"
    # button and its translations. Upstream has no SSO of any kind and no open
    # issue asking for it; this is written to be upstreamable as-is rather than
    # as a local hack, so it is off unless OIDC_ISSUER/CLIENT_ID/CLIENT_SECRET
    # are all set and it leaves the passkey path untouched.
    #
    # Regenerating after a version bump: clone the repo, `git checkout v<new>`,
    # `git apply` this patch, fix any rejects, `git diff > oidc.patch`. The two
    # places it is most likely to conflict are the route table in api/server.js
    # and the button stack in frontend/src/views/Login.jsx.
    patches = [ ./oidc.patch ];
  };

  meta = {
    description = "Self-hosted gym and body-weight tracker with passkey login";
    homepage = "https://gitlab.com/DuarteSantos8/opengym";
    license = lib.licenses.agpl3Plus;
    platforms = lib.platforms.linux;
  };
in
{
  opengym-api = buildNpmPackage {
    pname = "opengym-api";
    inherit version;
    src = "${src}/api";

    npmDepsHash = "sha256-KrJW6aaM5uzMZ7O1nJ7XVCnp4Da/qzX1r/1h8ojaQRM=";

    # server.js is the whole backend — there is nothing to compile, and no build
    # script in package.json for npm to run.
    dontNpmBuild = true;

    nativeBuildInputs = [ makeWrapper ];

    # package.json declares no `bin`, so npm installs the module and no
    # entrypoint. The unit wants one command to exec.
    postInstall = ''
      makeWrapper ${lib.getExe nodejs_24} $out/bin/opengym-api \
        --add-flags $out/lib/node_modules/gym-api/server.js
    '';

    meta = meta // { mainProgram = "opengym-api"; };
  };

  opengym-web = buildNpmPackage {
    pname = "opengym-web";
    inherit version meta;
    src = "${src}/frontend";

    npmDepsHash = "sha256-FogLxlDIAJuMcY4fb+4p/DCrxIK7VMKPZrdZfAM+xTw=";

    # @capacitor/assets (a devDependency, used only to generate icons for the
    # Android/iOS builds we do not make) depends on sharp, whose install script
    # tries to download libvips and write it into the read-only npm-deps store
    # path — which fails the build long before vite is reached. Nothing in the
    # web build needs any install script to have run.
    npmFlags = [ "--ignore-scripts" ];

    # Static output; no node_modules, no wrapper, nothing to run. nginx roots
    # the vhost here directly.
    installPhase = ''
      runHook preInstall
      cp -r dist $out
      runHook postInstall
    '';
  };

  # ~125 MB of exercise stills and animations. Upstream's compose file clones
  # this at first start into a bind mount; pinning the revision instead makes
  # the deployment reproducible and removes the first-boot network dependency.
  #
  # The pin is the exact revision upstream's own mobile build points its CDN at,
  # and it is checked: at v1.2.9 the 1324 image and 1324 GIF filenames in
  # frontend/src/lib/exercises-data.js all resolve here, with nothing spare.
  # Re-check that on any openGym bump — the filenames carry content hashes, so a
  # dataset that has moved on does not 404 loudly, it just shows blank tiles.
  #
  # Licensing: the metadata and instruction text are MIT, but the images and
  # animations are © Gym visual (https://gymvisual.com/) and are used under the
  # dataset's terms, not openGym's AGPL. See NOTICE.md in the openGym repo.
  # Reusing this media yourself, commercially or not, needs your own licence.
  # nixcache.nel.family is a pull-through proxy for cache.nixos.org and never
  # publishes locally-built paths, so this stays on our own machines.
  opengym-media = fetchFromGitHub {
    name = "opengym-media";
    owner = "hasaneyldrm";
    repo = "exercises-dataset";
    rev = "7455efae41b330c265e7cd4b78dfa848e7ce5ebd";
    hash = "sha256-bAit6zzd1Q1SPgb3ydjuZN78yXjRcgcIs+hH4gKNaxE=";
    meta = meta // {
      description = "Exercise images and animations used by openGym";
      license = lib.licenses.unfree;
    };
  };
}
