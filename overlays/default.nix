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

    # Wrap claude-code with extra tools it needs on PATH.
    claude-code = prev.claude-code.overrideAttrs (oldAttrs: {
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
