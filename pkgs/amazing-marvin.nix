{ lib, appimageTools, fetchurl }:

let
  pname = "amazing-marvin";
  version = "1.67.1";
  name = "${pname}-${version}";

  src = fetchurl {
    url = "https://amazingmarvin.s3.amazonaws.com/Marvin-${version}.AppImage";
    sha256 = "sha256-Tz9eD3fz0ULTGTyrVwvHloBIYeSmEW+5JLNksVQ7m2M=";
  };

  appimageContents = appimageTools.extractType2 { inherit pname version src; };
in
appimageTools.wrapType2 {
  inherit pname version name src;

  extraPkgs = pkgs:
    with pkgs; [
      xorg.libXScrnSaver
      xorg.libXtst
      libappindicator-gtk2
      libnotify
    ];

  extraInstallCommands = ''
    ls $out
    ls $out/bin
    install -m 444 -D ${appimageContents}/marvin.desktop $out/share/applications/marvin.desktop
    install -m 444 -D ${appimageContents}/usr/share/icons/hicolor/512x512/apps/marvin.png \
      $out/share/icons/hicolor/512x512/apps/marvin.png
    substituteInPlace $out/share/applications/marvin.desktop \
      --replace 'Exec=AppRun' 'Exec=${pname}'
  '';

  meta = with lib; {
    description = "Feature rich and customizable personal to-do app.";
    homepage = "https://amazingmarvin.com/";
    license = licenses.unfree;
    platforms = [ "x86_64-linux" ];
  };
}
