# This file defines overlays
{ inputs, ... }:
{
  # This one brings our custom packages from the 'pkgs' directory
  additions = final: _prev: import ../pkgs final.pkgs;

  # This one contains whatever you want to overlay
  # You can change versions, add patches, set compilation flags, anything really.
  # https://nixos.wiki/wiki/Overlays
  modifications = final: prev: {
    # libsForQt5.sddm = nixpkgs-unstable.libsForQt5.sddm;

    # Codex releases faster than nixpkgs can currently package them. Use the
    # official release binary while the channel is behind, then fall back to
    # nixpkgs automatically once it catches up.
    codex =
      let
        pinnedVersion = "0.153.4";
        targets = {
          x86_64-linux = {
            target = "x86_64-unknown-linux-musl";
            sha256 = "f479424eca092484dc40d87ae28c44f4cc40234a60045d6131e493800d814a30";
            codeModeHostSha256 = "f95830a869590957664bbfc67bccb08773806b693670baf15908176f89b4cd31";
          };
          aarch64-linux = {
            target = "aarch64-unknown-linux-musl";
            sha256 = "5cda6182bd94c3a30f2eb63a495489ebf7f691fddb14d70f48c6c1a5071b6cde";
            codeModeHostSha256 = "d8047b8d33370d6090e729d27eb76de60a2686baa1c143c138c9b05dc70d813b";
          };
          x86_64-darwin = {
            target = "x86_64-apple-darwin";
            sha256 = "d69200f0bf841b1d1a07f80b80cf742a2e4fc2bab91ae8a44b1042f8e8ca9fa4";
            codeModeHostSha256 = "2ffaebd0103d976232c358419a508859da862e128f3ca0bb071541346fbe3bf7";
          };
          aarch64-darwin = {
            target = "aarch64-apple-darwin";
            sha256 = "8cf911ea676523bfb2121ec561848d2aba564890ad536db4d8a3353f2b9850b1";
            codeModeHostSha256 = "45a9b0fdf53b98b85a6bb91e175dd90e961328a7a14fb50a40902205199df1df";
          };
        };
        platform = targets.${final.stdenv.hostPlatform.system};
        usePin = builtins.compareVersions prev.codex.version pinnedVersion < 0;
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

    # code-cursor pinned ahead of the channel. Cursor is closed source -- the
    # nixpkgs package only repackages the vendor AppImage -- so a bump is just a
    # newer `src` plus the version-derived `sourceRoot` that
    # appimageTools.extract lands the tree in. To resolve a new release:
    #
    #   curl -s https://api2.cursor.sh/updates/api/download/stable/linux-x64/cursor \
    #     | jq -r .downloadUrl
    #
    # then `nix-prefetch-url --type sha256 <url>` per platform (the arm64 URL
    # shares the same build hash). As with claude-code above, the pin applies
    # only while it is newer than the channel, so it no-ops once nixpkgs
    # catches up.
    code-cursor =
      let
        pinnedVersion = "3.20.10";
        build = "d6f462cdd0a6a6d1cff570daf980e671d0a63ded";
        sources = {
          x86_64-linux = {
            arch = "x64";
            appimageArch = "x86_64";
            hash = "sha256-zCY0PNenWzX5U6nXF7okUFz+GzV3E7vKV69llfdWhFA=";
          };
          aarch64-linux = {
            arch = "arm64";
            appimageArch = "aarch64";
            hash = "sha256-nfd88U0y44nDd7gnHcfyIajV53Jn30pBB5MzeoU1FTk=";
          };
        };
        system = final.stdenv.hostPlatform.system;
        source = sources.${system};
        usePin =
          sources ? ${system}
          && builtins.compareVersions prev.code-cursor.version pinnedVersion < 0;
      in
      if !usePin then
        prev.code-cursor
      else
        prev.code-cursor.overrideAttrs (_: {
          version = pinnedVersion;
          src = final.appimageTools.extract {
            pname = "cursor";
            version = pinnedVersion;
            src = final.fetchurl {
              url = "https://downloads.cursor.com/production/${build}/linux/${source.arch}/Cursor-${pinnedVersion}-${source.appimageArch}.AppImage";
              inherit (source) hash;
            };
          };
          sourceRoot = "cursor-${pinnedVersion}-extracted/usr/share/cursor";
        });

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
    stable = import inputs.nixpkgs24-05 {
      inherit (final.stdenv.hostPlatform) system;
      config = {
        allowUnfree = true;
      };
    };
  };
}
