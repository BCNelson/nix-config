# This file defines overlays
{ inputs, ... }:
{
  # This one brings our custom packages from the 'pkgs' directory
  # plus herdr-agent-parser, packaged in the herdr-android repo.
  additions = final: prev: import ../pkgs final.pkgs // inputs.herdr-android.overlays.default final prev;

  # Fixes, patches and wrappers on nixpkgs packages.
  #
  # Newer versions than nixpkgs has do NOT go here: add ./bumps/<attr>.nix
  # instead (see ./bumps/default.nix). Those are composed in front, so `prev`
  # below already holds whichever version won.
  modifications = inputs.nixpkgs-unstable.lib.composeExtensions (import ./bumps { inherit inputs; }) (final: prev: {
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

    # Wrap claude-code with extra tools it needs on PATH (any version, bumped
    # or not).
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

    # 7.3.4-zen1 panics at boot: its sched-alt merge left a second, unguarded
    # `worker->current_at = worker->task->...` line after the
    # `if (worker->task)` in process_one_work(). BH workqueue workers have no
    # task, so the first BH work item (on sierra, xhci_pci registering the USB 3
    # root hub) dereferences NULL+0x138 in ksoftirqd. Fixed in 7.3.4-zen2
    # (zen-kernel#425, nixpkgs#586820), which reached nixos-unstable-small and
    # its binary cache days before nixos-unstable did.
    #
    # Swap in -small's package set whole rather than overrideAttrs'ing zen1 to
    # the zen2 source: any override changes the store path, and then nothing
    # has it cached and every zen host compiles the kernel. The version string
    # is "7.3.4" for both, so match on modDirVersion. Once the channel moves
    # past zen1 this hands back to it by itself; delete it then.
    linuxPackages_zen =
      if prev.linuxPackages_zen.kernel.modDirVersion == "7.3.4-zen1" then
        inputs.nixpkgs-unstable-small.legacyPackages.${final.stdenv.hostPlatform.system}.linuxPackages_zen
      else
        prev.linuxPackages_zen;
  });

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
