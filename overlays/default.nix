# This file defines overlays
{ inputs, ... }:
{
  # This one brings our custom packages from the 'pkgs' directory
  # plus herdr-agent-parser, packaged in the herdr-android repo.
  additions = final: prev: import ../pkgs final.pkgs // inputs.herdr-android.overlays.default final prev;

  # This one contains whatever you want to overlay
  # You can change versions, add patches, set compilation flags, anything really.
  # https://nixos.wiki/wiki/Overlays
  modifications = final: prev: {
    # libsForQt5.sddm = nixpkgs-unstable.libsForQt5.sddm;

    # Codex releases faster than nixpkgs can currently package them. Use the
    # official release binary while the channel is behind, then fall back to
    # nixpkgs automatically once it overtakes the pin.
    #
    # The pin also wins a tie: nixpkgs' 0.167.0 source build fails in
    # aws-lc-sys ("AWS_LC_SYS_USE_SYSTEM=1 requires a system-installed
    # AWS-LC, but none was found" -- it only finds openssl), so nothing
    # caches it and every host that pulls codex fails to build. Hand-off to
    # nixpkgs therefore happens only once the channel ships a *newer* codex;
    # check that one builds before letting it through.
    codex =
      let
        pinnedVersion = "0.167.0";
        targets = {
          x86_64-linux = {
            target = "x86_64-unknown-linux-musl";
            sha256 = "9d64836106d5b374e90ee4454c7ebcde7ca7106558bd7e41a34b9f26ebda251a";
            codeModeHostSha256 = "b5074d5dd64643b2c073927e494a588eff31c1f5a27f12c1e2c45f9a6cf07dd4";
          };
          aarch64-linux = {
            target = "aarch64-unknown-linux-musl";
            sha256 = "843489acb4d4b1079745c36b03c5963b887b83155af208fd53584e33ee8eb281";
            codeModeHostSha256 = "6e99948fa20ca816a01f222a56afa261b321e00cbed611d76bb92c765fd11ec6";
          };
          x86_64-darwin = {
            target = "x86_64-apple-darwin";
            sha256 = "d8eb4a865a0b17319cde10347523e5bab0978d01725b39f4a639f40905e01221";
            codeModeHostSha256 = "612d0dc2c7316fbf05ee922796f4d740a285b76a55d4c78e1b946ba621bdcc91";
          };
          aarch64-darwin = {
            target = "aarch64-apple-darwin";
            sha256 = "4b22a2a971be2791c3e31f912f3e77a88ebd64a65812a7289ccba65dd6425fe9";
            codeModeHostSha256 = "876a3e263348ef0edaeae5cf5bc52a5c121141cefd15157ab0eeca305f3eb563";
          };
        };
        platform = targets.${final.stdenv.hostPlatform.system};
        usePin = builtins.compareVersions prev.codex.version pinnedVersion <= 0;
      in
      if usePin then
        final.stdenvNoCC.mkDerivation {
          pname = "codex";
          version = pinnedVersion;

          src = final.fetchurl {
            url = "https://github.com/openai/codex/releases/download/rust-v${pinnedVersion}/codex-${platform.target}.tar.gz";
            inherit (platform) sha256;
          };
          codeModeHostSrc = final.fetchurl {
            url = "https://github.com/openai/codex/releases/download/rust-v${pinnedVersion}/codex-code-mode-host-${platform.target}.tar.gz";
            sha256 = platform.codeModeHostSha256;
          };

          dontUnpack = true;
          nativeBuildInputs = [ final.makeWrapper ];

          installPhase = ''
            runHook preInstall
            mkdir -p "$out/bin"
            tar -xzf "$src"
            tar -xzf "$codeModeHostSrc"
            install -m755 "codex-${platform.target}" "$out/bin/codex"
            install -m755 "codex-code-mode-host-${platform.target}" "$out/bin/codex-code-mode-host"
            wrapProgram "$out/bin/codex" \
              --prefix PATH : ${final.lib.makeBinPath (
                [ final.ripgrep ]
                ++ final.lib.optionals final.stdenv.hostPlatform.isLinux [ final.bubblewrap ]
              )}
            runHook postInstall
          '';

          inherit (prev.codex) meta;
        }
      else
        prev.codex;

    # gdal 3.13.1's zarr sharding test expects a `zarr.json.gmac` sidecar that
    # isn't produced in the `useMinimalFeatures = true` build (pulled in by
    # vtk -> freecad), so it fails with `assert None is not None` in
    # gdrivers/zarr_driver.py. Deselect just that test to unblock the build.
    # overrideAttrs survives vtk's `.override { useMinimalFeatures = true; }`.
    gdal = prev.gdal.overrideAttrs (old: {
      disabledTestPaths = (old.disabledTestPaths or [ ]) ++ [
        "gdrivers/zarr_driver.py::test_zarr_read_simple_sharding"
      ];
    });

    # urllib3-future 2.25.902 fails its own suite in the sandbox: 34 failures,
    # all in test/contrib/test_socks.py (MaxRetryError / ConnectTimeoutError
    # against the SOCKS dummy servers). Hydra hits the same thing, so nothing
    # caches it, and the suite takes ~95 minutes before failing -- longer on
    # romeo, which builds it for niquests -> caldav -> odysseus-ai. Skip the
    # tests rather than deselecting the SOCKS file, so the build is minutes
    # instead of an hour and a half; pythonImportsCheck still runs. Drop this
    # once Hydra builds it again (hydra.nixos.org job
    # nixpkgs/unstable/python314Packages.urllib3-future.x86_64-linux).
    #
    # niquests 3.21.2 on top of it then fails test_use_proxy_from_environment
    # (all 8 variants: the fake proxy never sees a connection). Hydra never got
    # that far -- niquests and caldav show "dependency failed" there. Together
    # with the SOCKS failures this looks like a real proxy regression in this
    # urllib3-future, not sandbox noise. odysseus-ai (the only consumer, on
    # romeo) runs with no *_proxy variables, so it is not affected; anything
    # that starts relying on proxies through niquests should re-check this.
    pythonPackagesExtensions = prev.pythonPackagesExtensions ++ [
      (_pyFinal: pyPrev: {
        urllib3-future = pyPrev.urllib3-future.overridePythonAttrs { doCheck = false; };
        niquests = pyPrev.niquests.overridePythonAttrs (old: {
          disabledTests = (old.disabledTests or [ ]) ++ [ "test_use_proxy_from_environment" ];
        });
      })
    ];

    # patchelf 0.19.2 (now stdenv's) writes a broken layout when it rewrites
    # both the interpreter and the rpath: .dynamic lands in a read-only LOAD
    # segment and ld.so segfaults in dl_main before the program runs
    # (NixOS/patchelf#674, nixpkgs#576412). trilium-server autoPatchelfs the
    # prebuilt node it ships, so the service dumped core on every start after
    # the bump. auto-patchelf runs whatever patchelf is first on PATH; put a
    # 0.15 one there. patchelfUnstable (0.18) is not old enough -- it produces
    # the same overlapping LOADs -- so borrow 0.15.2 from the
    # `nixpkgs-patchelf-0_15` input, pinned by rev to a nixos-unstable from
    # before the bump (the plain `nixpkgs` input took it on 2026-10-21 and the
    # daily flake bump has failed on this assert since). The assert makes this
    # fail at evaluation, rather than ship a binary that segfaults, if that pin
    # is ever moved past the bump. Drop this once nixpkgs#576412 is fixed.
    trilium-server =
      let
        patchelf = inputs.nixpkgs-patchelf-0_15.legacyPackages.${final.stdenv.buildPlatform.system}.patchelf;
      in
      assert final.lib.versionOlder patchelf.version "0.16";
      prev.trilium-server.overrideAttrs (old: {
        nativeBuildInputs = [ patchelf ] ++ (old.nativeBuildInputs or [ ]);
      });

    # nixpkgs' cargoSetupHook now forces -sys crates onto the system copies of
    # common C libraries (ZSTD_SYS_USE_PKG_CONFIG=1 among them; 26.11 release
    # notes), but herdr-desktop 0.1.10 never got zstd in its buildInputs, so
    # zstd-sys panics with "Can't probe for zstd in pkg-config". Supply it.
    # Drop this once nixpkgs' herdr-desktop lists zstd itself.
    herdr-desktop = prev.herdr-desktop.overrideAttrs (old: {
      buildInputs = (old.buildInputs or [ ]) ++ [ final.zstd ];
    });

    # Same hook, LIBSQLITE3_SYS_USE_PKG_CONFIG=1: voxtype 1.1.1 has no sqlite
    # in its buildInputs, so its binaries fail to link (`cannot find
    # -lsqlite3`). voxtype-vulkan (used on redo-3) inherits this override.
    # Drop once nixpkgs' voxtype lists sqlite itself.
    voxtype = prev.voxtype.overrideAttrs (old: {
      buildInputs = (old.buildInputs or [ ]) ++ [ final.sqlite ];
    });

    # GAM 7.43.04 pins chardet==5.2.0 in its pyproject, but nixpkgs now ships
    # chardet 6.0.0, so pythonRuntimeDepsCheckHook fails with
    # `chardet==5.2.0 not satisfied by version 6.0.0.post1`. Relax the pin;
    # chardet 6.x is API-compatible for GAM's CSV encoding detection.
    # Setting pythonRelaxDeps via overrideAttrs works because gam uses the
    # finalAttrs fixpoint, so mk-python-derivation auto-adds pythonRelaxDepsHook.
    gam = prev.gam.overrideAttrs (old: {
      pythonRelaxDeps = (old.pythonRelaxDeps or [ ]) ++ [ "chardet" ];
    });

    # mongodb-compass builds via a custom `buildCommand`, which skips the
    # standard fixupPhase and so calls `wrapGAppsHook` manually. nixpkgs
    # redesigned wrapGAppsHook to be output-aware (it indexes
    # `wrapGAppsHookHasRunForOutput["$output"]` and auto-discovers binaries in
    # `$prefix/bin`), but inside buildCommand neither `$output` nor `$prefix`
    # is set, so the empty associative-array subscript aborts with
    # `wrapGAppsHookHasRunForOutput: bad array subscript`. The new hook also no
    # longer takes a program-path argument. Set output/prefix and drop the arg.
    mongodb-compass = prev.mongodb-compass.overrideAttrs (old: {
      buildCommand = builtins.replaceStrings
        [ "wrapGAppsHook $out/bin/mongodb-compass" ]
        [ "output=out prefix=\"$out\" wrapGAppsHook" ]
        old.buildCommand;
    });

    # Wrap claude-code with extra tools it needs on PATH.
    #
    # We also carry a pinned bump (nixpkgs PR #545319, a plain version bump).
    # The package fetches a prebuilt binary keyed by version + per-platform
    # checksum, so overriding version and src with the PR's manifest values is
    # enough. The pin is only applied when it's *newer* than what the channel
    # already ships, so whichever version is later wins and the override
    # becomes a no-op automatically once the channel catches up.
    claude-code =
      let
        pinnedVersion = "2.1.219";
        # sha256 checksums from the PR's manifest.json, per node platform-arch key.
        checksums = {
          "linux-x64" = "22cfd6f5b3061c0391ba84e9cf8c9deaa37783aac18b004d42ec061e98f00691";
          "linux-arm64" = "1f834b322ba9d1291cc7ffeff16a6795a59145bda279dbd59cd7ecebc7b7f15a";
          "darwin-arm64" = "a8e806faaefac53c7a0f26523d8a45c60dbef3407b14ef990c75765d08febc82";
        };
        platformKey = "${final.stdenv.hostPlatform.node.platform}-${final.stdenv.hostPlatform.node.arch}";
        # Only override version/src when the channel's claude-code is older.
        usePin = builtins.compareVersions prev.claude-code.version pinnedVersion < 0;
        versionOverride = final.lib.optionalAttrs usePin {
          version = pinnedVersion;
          src = final.fetchurl {
            url = "https://downloads.claude.ai/claude-code-releases/${pinnedVersion}/${platformKey}/claude";
            sha256 = checksums.${platformKey};
          };
        };
      in
      prev.claude-code.overrideAttrs (oldAttrs: versionOverride // {
      postFixup = (oldAttrs.postFixup or "") + ''
        wrapProgram $out/bin/claude \
          --prefix PATH : ${final.lib.makeBinPath [
            final.coreutils-full
            final.findutils
            final.gnumake
            final.gnused
            final.gnugrep
            final.bash
            final.sox
          ]}
      '';
    });

    # Cursor's stable channel runs several releases ahead of nixpkgs, so we
    # carry our own build of it in ../pkgs/code-cursor (see that file for what
    # "our own build" means for a closed-source AppImage).
    #
    # Whichever tree is newer wins. When nixpkgs catches up or overtakes us --
    # or on a system our package does not claim (darwin) -- `pkgs.code-cursor`
    # is the channel's build again and ../pkgs/code-cursor stops being built at
    # all. To get back ahead, run ../pkgs/code-cursor/update.sh.
    code-cursor =
      let
        ours = final.callPackage ../pkgs/code-cursor { };
        supported = builtins.elem final.stdenv.hostPlatform.system ours.meta.platforms;
      in
      if supported && builtins.compareVersions ours.version prev.code-cursor.version > 0 then
        ours
      else
        prev.code-cursor;

    # happy-coder pinned to nixpkgs PR #492656 (monorepo migration) until it
    # lands in unstable. Brings 1.1.x without the bundled @anthropic-ai/claude-code
    # 2.0.14 that crashes with `Cannot read properties of null (reading
    # 'alwaysThinking')` on first message (anthropics/claude-code#52225).
    # The PR's wrapper invokes node by absolute path, but the CLI spawns child
    # `node` processes via PATH lookup (e.g. `happy daemon start-sync`), so
    # add nodejs to PATH.
    happy-coder = inputs.nixpkgs-happy-coder.legacyPackages.${final.stdenv.hostPlatform.system}.happy-coder.overrideAttrs (old: {
      nativeBuildInputs = (old.nativeBuildInputs or []) ++ [ final.makeWrapper ];
      postFixup = (old.postFixup or "") + ''
        wrapProgram $out/bin/happy \
          --prefix PATH : ${final.lib.makeBinPath [ final.nodejs ]}
        wrapProgram $out/bin/happy-mcp \
          --prefix PATH : ${final.lib.makeBinPath [ final.nodejs ]}
      '';
    });
  };

  # When applied, the unstable nixpkgs set (declared in the flake inputs) will
  # be accessible through 'pkgs.unstable'
  unstable-packages = final: _prev: {
    unstable = import inputs.nixpkgs-unstable {
      inherit (final.stdenv.hostPlatform) system;
      config = {
        allowUnfree = true;
        permittedInsecurePackages = [
          "electron-25.9.0"
          "libsoup-2.74.3"
        ];
      };
    };
  };
}
