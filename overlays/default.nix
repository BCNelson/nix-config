# This file defines overlays
{ inputs, ... }:
{
  # This one brings our custom packages from the 'pkgs' directory
  additions = final: _prev: (import ../pkgs final.pkgs) // {
    # pkgs/default.nix only receives `pkgs`, and herdr-web's source comes from a
    # flake input rather than a hash we maintain, so it is wired up here where
    # `inputs` is in scope.
    herdr-web = final.callPackage ../pkgs/herdr-web { src = inputs.herdr-web; };
  };

  # This one contains whatever you want to overlay
  # You can change versions, add patches, set compilation flags, anything really.
  # https://nixos.wiki/wiki/Overlays
  modifications = final: prev: {
    # libsForQt5.sddm = nixpkgs-unstable.libsForQt5.sddm;

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
