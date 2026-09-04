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
  libsecret,
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
  xdg-utils,
}:
# Grok Bot is xAI's desktop coding agent, shipped by the Cursor team (the .deb's
# maintainer is "SpaceXAI <hi@cursor.com>" and it Provides/Replaces "sand", its
# former codename). Not in nixpkgs, and there is no source release -- only the
# prebuilt Electron tree, so this unpacks the vendor .deb and autoPatchelfs it,
# same as ./goose-desktop.nix.
#
# `src` is the release artifact the update channel currently points at. To bump:
#
#   curl -s https://api2.cursor.sh/updates/api/download/stable/linux-x64/sand \
#     | jq -r .debUrl
#
# (the channel still calls the app by its "sand" codename; "grok-bot" is
# rejected by that endpoint). Pin the resolved downloads.cursor.com URL, which
# is keyed by build commit -- not the channel endpoint, which is a moving
# target.
stdenv.mkDerivation (finalAttrs: {
  pname = "grok-bot";
  version = "0.39.0";

  src = fetchurl {
    url = "https://downloads.cursor.com/grokbot/stable/d8bc9c753edddb313047c9c69b480b7f8f321087/linux/x64/grok-bot_${finalAttrs.version}_amd64.deb";
    hash = "sha256-rkmK2vcfn/FzSnhhY8WAf6UBZay1qhArto9D6tlt5P4=";
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
    libsecret
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

  # The app tree lives in `/opt/Grok Bot/`. Drop the space on the way in so no
  # downstream wrapper, .desktop Exec line or RPATH has to quote it.
  installPhase = ''
    runHook preInstall

    mkdir -p $out/lib $out/share
    cp -r "opt/Grok Bot" $out/lib/grok-bot
    cp -r usr/share/applications $out/share/
    cp -r usr/share/icons $out/share/

    runHook postInstall
  '';

  # chrome-sandbox needs to be setuid root, which a Nix store path can never be.
  # Removing it (rather than passing --no-sandbox) keeps the renderer sandbox
  # intact -- only the setuid fallback goes away, and Electron falls back to the
  # unprivileged userns sandbox, which redo-3's Fedora kernel allows by default.
  #
  # xdg-utils is on PATH for `xdg-open` (link/file opening); the .deb depends on
  # it. coreutils covers the shell helpers the agent shells out to.
  postFixup = ''
    rm -f $out/lib/grok-bot/chrome-sandbox

    makeWrapper $out/lib/grok-bot/grok-bot $out/bin/grok-bot \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath [libGL libgbm libsecret]}" \
      --prefix PATH : "${lib.makeBinPath [coreutils xdg-utils]}" \
      "''${gappsWrapperArgs[@]}"

    substituteInPlace $out/share/applications/grok-bot.desktop \
      --replace-fail "Exec=grok-bot" "Exec=$out/bin/grok-bot"
  '';

  # wrapGAppsHook3 would otherwise also wrap $out/lib/grok-bot/grok-bot directly
  # and double-wrap the binary our makeWrapper call targets.
  dontWrapGApps = true;

  meta = {
    description = "Grok Bot desktop coding agent";
    homepage = "https://cursor.com";
    license = lib.licenses.unfree;
    mainProgram = "grok-bot";
    platforms = ["x86_64-linux"];
    sourceProvenance = [lib.sourceTypes.binaryNativeCode];
  };
})
