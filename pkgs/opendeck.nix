{
  lib,
  stdenv,
  fetchurl,
  dpkg,
  autoPatchelfHook,
  wrapGAppsHook3,
  buildFHSEnv,
  webkitgtk_4_1,
  gtk3,
  glib,
  glib-networking,
  libsoup_3,
  gdk-pixbuf,
  librsvg,
  cairo,
  pango,
  harfbuzz,
  openssl,
  libayatana-appindicator,
  libappindicator-gtk3,
}:
# OpenDeck is a Tauri app and is not in nixpkgs. We build from the upstream
# .deb rather than the .AppImage: the .deb links against the *system*
# webkit2gtk, so autoPatchelf'ing it against nixpkgs' own webkitgtk gives us
# a working GL/EGL stack. The AppImage instead bundles its own WebKit engine,
# which cannot initialise EGL inside the Nix FHS sandbox (white screen).
let
  opendeck-unwrapped = stdenv.mkDerivation rec {
    pname = "opendeck-unwrapped";
    version = "2.13.1";

    src = fetchurl {
      url = "https://github.com/ninjadev64/OpenDeck/releases/download/v${version}/opendeck_${version}_amd64.deb";
      hash = "sha256-4Trt0v+8GeVR2e+vSwND0gbIFCePs/mIPVE5igaHbyg=";
    };

    nativeBuildInputs = [dpkg autoPatchelfHook wrapGAppsHook3];

    buildInputs = [
      webkitgtk_4_1
      gtk3
      glib
      libsoup_3
      gdk-pixbuf
      librsvg
      cairo
      pango
      harfbuzz
      openssl
      libayatana-appindicator
      libappindicator-gtk3
    ];

    # webkit's networking (TLS) is provided as a glib gio module.
    propagatedBuildInputs = [glib-networking];

    # OpenDeck dlopen()s the appindicator library by soname at runtime (for its
    # tray icon), so it isn't picked up by autoPatchelf's DT_NEEDED scan. Put it
    # on the wrapped binary's LD_LIBRARY_PATH so the dlopen succeeds.
    preFixup = ''
      gappsWrapperArgs+=(
        --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath [libayatana-appindicator]}"
      )
    '';

    unpackPhase = ''
      runHook preUnpack
      dpkg-deb -x $src .
      runHook postUnpack
    '';

    installPhase = ''
      runHook preInstall

      mkdir -p $out
      cp -r usr/bin usr/lib usr/share $out/

      runHook postInstall
    '';

    # wrapGAppsHook3 wraps $out/bin/opendeck with the right GIO/GDK/GSettings env
    # (icons, glib-networking TLS, gdk-pixbuf loaders) so the webview renders.
  };
in
  # OpenDeck downloads plugins into ~/.config/opendeck/plugins/ at runtime and
  # launches them as child processes. Those plugins are generic dynamically
  # linked ELF binaries (e.g. built for x86_64-unknown-linux-gnu) that expect a
  # standard /lib64/ld-linux loader and common shared libraries — which NixOS
  # doesn't provide out of the box. Wrapping the whole app in an FHS env means
  # every child process (plugins included) runs inside a bubblewrap chroot where
  # the loader and libraries exist, so the plugins start correctly.
  buildFHSEnv {
    pname = "opendeck";
    inherit (opendeck-unwrapped) version;

    # The FHS /usr tree the app and its plugins see. opendeck-unwrapped itself is
    # already autopatchelf'd against /nix/store, so it keeps rendering natively;
    # the rest is the baseline set a downloaded plugin is likely to link against.
    targetPkgs = pkgs: (with pkgs; [
      opendeck-unwrapped

      # C/C++ runtime + common libs plugins are typically linked against.
      stdenv.cc.cc.lib # libstdc++, libgcc_s
      glibc
      zlib
      openssl
      curl
      libusb1 # HID/USB access (Stream Deck plugins)

      # GUI stack, in case a plugin ships its own GTK/webkit UI.
      gtk3
      glib
      gdk-pixbuf
      cairo
      pango
      libGL
      libxkbcommon # keyboard handling (opendeck-starterpack links this)
      wayland
      fontconfig
      freetype
      dbus
      # X11 libs commonly pulled in by GUI/windowing toolkits.
      libx11
      libxcursor
      libxi
      libxrandr
      libxcb
    ]);

    runScript = "opendeck";

    # Surface the desktop entry and icons through the FHS wrapper so the app shows
    # up in the launcher, pointing Exec at the wrapped `opendeck` on PATH.
    extraInstallCommands = ''
      mkdir -p $out/share
      cp -r ${opendeck-unwrapped}/share/applications $out/share/
      cp -r ${opendeck-unwrapped}/share/icons $out/share/ 2>/dev/null || true
      cp -r ${opendeck-unwrapped}/share/pixmaps $out/share/ 2>/dev/null || true
    '';

    meta = with lib; {
      description = "Cross-platform desktop application that controls Elgato Stream Deck devices";
      homepage = "https://opendeck.rest/";
      license = licenses.gpl3Only;
      mainProgram = "opendeck";
      platforms = ["x86_64-linux"];
    };
  }
