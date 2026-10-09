# Spool: clipboard manager for KDE Plasma 6 on Wayland.
# Builds the daemon (spoold), CLI (spoolctl), the offline key-slot tool
# (spool-keyctl) and the auto-paste helper (spool-paster) from the in-tree
# workspace (`--workspace` installs every bin except the picker). The Slint
# picker is the separate `spool-picker` package (picker.nix) so the daemon
# closure stays small.
{
  lib,
  rustPlatform,
  pkg-config,
  sqlcipher,
  libxkbcommon,
  libfido2,
  openssl,
  zlib,
}: let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
in
  rustPlatform.buildRustPackage {
    pname = "spool";
    inherit (cargoToml.workspace.package) version;

    # Only the files cargo needs, so target/, README edits etc. don't cause
    # rebuilds.
    src = lib.fileset.toSource {
      root = ./.;
      fileset = lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./crates
        ./kwin-script
      ];
    };

    cargoLock.lockFile = ./Cargo.lock;

    # rusqlite's `sqlcipher` feature links the system SQLCipher via pkg-config.
    # wayland-client 0.31 is pure Rust, so no libwayland is needed.
    # spool-keys' FIDO2 provider links libfido2 through fido2-rs/libfido2-sys,
    # whose build script runs bindgen (libclang via bindgenHook).
    nativeBuildInputs = [pkg-config rustPlatform.bindgenHook];
    # spool-paste resolves paste keycodes with libxkbcommon (its unit tests
    # compile keymaps from the xkeyboard-config data libxkbcommon points at).
    buildInputs = [sqlcipher libxkbcommon libfido2 openssl zlib];

    # The Slint picker is its own package (picker.nix) so the daemon closure
    # never pulls Slint.
    cargoBuildFlags = ["--workspace" "--exclude" "spool-picker"];

    # Unit tests only; nothing here needs a Wayland compositor.
    doCheck = true;
    cargoTestFlags = ["--workspace" "--exclude" "spool-picker"];

    # KWin script (loaded by spoold over D-Bus), spoold's desktop file (the
    # portal app id), and the one that lets KWin grant the spool-paster
    # helper org_kde_kwin_fake_input (matched on the Exec path; spoold itself
    # is non-dumpable, so KWin cannot match it).
    postInstall = ''
      mkdir -p $out/share/spool $out/share/applications
      cp -r kwin-script $out/share/spool/kwin-script
      substitute kwin-script/dev.bcnelson.spool.daemon.desktop.in \
        $out/share/applications/dev.bcnelson.spool.daemon.desktop \
        --subst-var-by spoold $out/bin/spoold
      substitute kwin-script/dev.bcnelson.spool.paster.desktop.in \
        $out/share/applications/dev.bcnelson.spool.paster.desktop \
        --subst-var-by paster $out/bin/spool-paster
      rm $out/share/spool/kwin-script/*.desktop.in
    '';

    meta = {
      description = "Clipboard history manager for KDE Plasma 6 on Wayland (daemon, CLI, key-slot tool)";
      license = lib.licenses.mit;
      maintainers = [];
      platforms = lib.platforms.linux;
      mainProgram = "spoolctl";
    };
  }
