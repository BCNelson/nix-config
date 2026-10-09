# Spool picker: Slint UI on a wlr-layer-shell surface, software-rendered into
# wl_shm buffers (optional FemtoVG/EGL renderer, `gpu` feature). spoold
# launches it once and drives it over a socketpair on fd 3
# (SPOOL_PICKER_FD=3). Kept out of the `spool` package so the daemon closure
# never pulls Slint.
{
  lib,
  rustPlatform,
  pkg-config,
  libxkbcommon,
  fontconfig,
  wayland,
  libglvnd,
}: let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
in
  rustPlatform.buildRustPackage {
    pname = "spool-picker";
    inherit (cargoToml.workspace.package) version;

    src = lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./crates
      ];
    };

    cargoLock.lockFile = ./Cargo.lock;

    # libxkbcommon: keymaps from the compositor (smithay-client-toolkit).
    # fontconfig: Slint's fallback font lookup (the UI font is embedded).
    # wayland: libwayland-client/-egl (the `gpu` feature uses the system
    # libwayland-client so EGL can share the connection).
    nativeBuildInputs = [pkg-config];
    buildInputs = [libxkbcommon fontconfig wayland];

    # `gpu`: FemtoVG/EGL renderer, off unless SPOOL_PICKER_RENDERER=gpu
    # (software is the default and the fallback).
    buildFeatures = ["gpu"];
    cargoBuildFlags = ["-p" "spool-picker"];
    cargoTestFlags = ["-p" "spool-picker"];

    # libEGL is dlopen()ed at runtime (a missing one = software fallback);
    # libglvnd finds the Mesa/NVIDIA vendor library via /run/opengl-driver.
    postFixup = ''
      patchelf --add-rpath ${lib.makeLibraryPath [libglvnd]} $out/bin/spool-picker
    '';

    # Unit tests only (sanitizer, kdeglobals, thumbnail limits, protocol
    # correlation); nothing here needs a compositor or a GPU.
    doCheck = true;

    meta = {
      description = "Clipboard history picker UI for Spool (launched by spoold)";
      # The binary links Slint, used under its GPL-3.0-only option, so the
      # picker as distributed is GPL-3.0-only (Spool's own code stays MIT,
      # GPL-compatible). The embedded UI font (Noto Sans subset) is OFL-1.1.
      license = with lib.licenses; [gpl3Only ofl];
      maintainers = [];
      platforms = lib.platforms.linux;
      mainProgram = "spool-picker";
    };
  }
