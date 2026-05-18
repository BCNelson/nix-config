{ lib
, stdenv
, fetchurl
, dpkg
, autoPatchelfHook
, makeWrapper
, wrapGAppsHook3
, alsa-lib
, at-spi2-atk
, at-spi2-core
, atk
, cairo
, cups
, dbus
, expat
, fontconfig
, freetype
, gdk-pixbuf
, glib
, gtk3
, libdrm
, libnotify
, libsecret
, libuuid
, libxkbcommon
, libgbm
, libGL
, libpulseaudio
, libappindicator-gtk3
, mesa
, nspr
, nss
, pango
, systemd
, vulkan-loader
, wayland
, xdotool
, libx11
, libxcb
, libxshmfence
, libxcomposite
, libxcursor
, libxdamage
, libxext
, libxfixes
, libxi
, libxrandr
, libxrender
, libxscrnsaver
, libxtst
}:

stdenv.mkDerivation rec {
  pname = "openhuman";
  version = "0.53.43";

  src = fetchurl {
    url = "https://github.com/tinyhumansai/openhuman/releases/download/v${version}/OpenHuman_${version}_amd64.deb";
    sha256 = "sha256-1U0JQy0qh/Or7WT+G1Avjl9ClTPOwTGwLVZj6qvOaWo=";
  };

  nativeBuildInputs = [
    dpkg
    autoPatchelfHook
    makeWrapper
    wrapGAppsHook3
  ];

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
    libnotify
    libsecret
    libuuid
    libxkbcommon
    libgbm
    libpulseaudio
    libappindicator-gtk3
    mesa
    nspr
    nss
    pango
    systemd
    xdotool
    libx11
    libxcb
    libxshmfence
    libxcomposite
    libxcursor
    libxdamage
    libxext
    libxfixes
    libxi
    libxrandr
    libxrender
    libxscrnsaver
    libxtst
  ];

  runtimeDependencies = [
    libGL
    vulkan-loader
    wayland
  ];

  unpackCmd = "${dpkg}/bin/dpkg-deb -x $curSrc source";
  sourceRoot = "source";

  dontBuild = true;
  dontConfigure = true;
  dontWrapGApps = true;

  installPhase = ''
    runHook preInstall

    mkdir -p $out
    cp -r usr/share $out/share
    cp -r usr/lib $out/lib

    substituteInPlace $out/share/applications/OpenHuman.desktop \
      --replace 'Exec=OpenHuman' 'Exec=openhuman'

    runHook postInstall
  '';

  postFixup = ''
    makeWrapper $out/share/OpenHuman/OpenHuman $out/bin/openhuman \
      "''${gappsWrapperArgs[@]}" \
      --prefix LD_LIBRARY_PATH : "${lib.makeLibraryPath [ libGL vulkan-loader wayland libpulseaudio ]}"
  '';

  meta = with lib; {
    description = "OpenHuman - AI-powered super assistant";
    homepage = "https://github.com/tinyhumansai/openhuman";
    license = licenses.unfree;
    platforms = [ "x86_64-linux" ];
    mainProgram = "openhuman";
  };
}
