{
  lib,
  stdenv,
  fetchurl,
  dpkg,
  autoPatchelfHook,
  makeWrapper,
  wrapGAppsHook3,
  alsa-lib,
  at-spi2-atk,
  at-spi2-core,
  atk,
  cairo,
  cups,
  dbus,
  expat,
  fontconfig,
  freetype,
  gdk-pixbuf,
  glib,
  gtk3,
  libdrm,
  libGL,
  libgbm,
  libnotify,
  libpulseaudio,
  libuuid,
  libxkbcommon,
  nspr,
  nss,
  pango,
  systemd,
  libx11,
  libxcb,
  libxcomposite,
  libxcursor,
  libxdamage,
  libxext,
  libxfixes,
  libxi,
  libxrandr,
  libxrender,
  libxscrnsaver,
  libxshmfence,
  libxtst,
  coreutils,
}:
# Goose Desktop is the Electron GUI. There is no browser-accessible web UI --
# goose 1.45 has no `web` subcommand; `goose serve` speaks ACP over HTTP/WS and
# this app is the client for it.
#
# Built from the upstream .deb rather than source: the repo builds the Electron
# shell with npm/electron-forge, which is a network-fetching build we cannot
# reproduce hermetically. The .deb ships a normal dynamically linked Electron
# tree, so autoPatchelfHook is enough.
#
# The app bundles its own `goose` CLI at resources/bin/goose plus node/npx/uvx/
# jbang for spawning MCP servers. Those stay as shipped: they are only used when
# the app runs its own local backend, which our wrapper disables (see
# GOOSE_EXTERNAL_BACKEND in the NixOS module).
stdenv.mkDerivation (finalAttrs: {
  pname = "goose-desktop";
  version = "1.45.0";

  src = fetchurl {
    url = "https://github.com/aaif-goose/goose/releases/download/v${finalAttrs.version}/goose_${finalAttrs.version}_amd64.deb";
    hash = "sha256-C4hr+rwP3rBQ/GSzY2u7828YNIERIGEWQaMo0vUSLQE=";
  };

  nativeBuildInputs = [dpkg autoPatchelfHook makeWrapper wrapGAppsHook3];

  buildInputs = [
    alsa-lib
    at-spi2-atk
    at-spi2-core
    atk
    cairo
    cups
    dbus
    expat
    fontconfig
    freetype
    gdk-pixbuf
    glib
    gtk3
    libdrm
    libGL
    libgbm
    libnotify
    libpulseaudio
    libuuid
    libxkbcommon
    nspr
    nss
    pango
    systemd # libudev
    stdenv.cc.cc.lib
    libx11
    libxcb
    libxcomposite
    libxcursor
    libxdamage
    libxext
    libxfixes
    libxi
    libxrandr
    libxrender
    libxscrnsaver
    libxshmfence
    libxtst
  ];

  # Not `dpkg-deb -x`: the archive stores chrome-sandbox as 4755, and restoring
  # the setuid bit fails inside the build sandbox ("Cannot change mode to
  # rwsr-xr-x"). Stream the payload through tar with permission restoration
  # disabled -- the helper is deleted in postFixup anyway.
  unpackPhase = ''
    runHook preUnpack
    dpkg-deb --fsys-tarfile $src | tar -x --no-same-permissions --no-same-owner
    runHook postUnpack
  '';

  installPhase = ''
    runHook preInstall

    mkdir -p $out/lib $out/share
    cp -r usr/lib/goose $out/lib/goose
    cp -r usr/share/applications $out/share/ 2>/dev/null || true
    cp -r usr/share/icons $out/share/ 2>/dev/null || true
    cp -r usr/share/pixmaps $out/share/ 2>/dev/null || true

    runHook postInstall
  '';

  # chrome-sandbox needs to be setuid root, which a Nix store path can never be.
  # NixOS gates the namespace sandbox behind
  # security.unprivilegedUsernsClone/chromiumSuidSandbox; rather than depend on
  # host config, drop the bundled helper and let Electron use the userns
  # sandbox. Removing it (instead of passing --no-sandbox) keeps the renderer
  # sandbox intact -- only the setuid fallback path goes away.
  postFixup = ''
    rm -f $out/lib/goose/chrome-sandbox

    makeWrapper $out/lib/goose/Goose $out/bin/goose-desktop \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath [libGL libgbm]}" \
      --prefix PATH : "${lib.makeBinPath [coreutils]}" \
      "''${gappsWrapperArgs[@]}"

    substituteInPlace $out/share/applications/goose.desktop \
      --replace-quiet "Exec=/usr/lib/goose/Goose" "Exec=$out/bin/goose-desktop" \
      --replace-quiet "Exec=goose" "Exec=goose-desktop"
  '';

  # wrapGAppsHook3 would otherwise also wrap $out/lib/goose/Goose directly and
  # double-wrap the binary our makeWrapper call targets.
  dontWrapGApps = true;

  meta = {
    description = "Electron desktop client for the goose AI agent";
    homepage = "https://goose-docs.ai/";
    license = lib.licenses.asl20;
    mainProgram = "goose-desktop";
    platforms = ["x86_64-linux"];
    sourceProvenance = [lib.sourceTypes.binaryNativeCode];
  };
})
