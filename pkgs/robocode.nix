{ lib, stdenvNoCC, fetchurl, jdk, makeWrapper }:

# Robocode Tank Royale GUI. Distributed upstream only as a runnable jar (plus
# native installers we can't use on NixOS); we fetch the jar and wrap it with a
# JDK. https://robocode.dev/articles/installing-robocode.html
stdenvNoCC.mkDerivation (finalAttrs: {
  pname = "robocode";
  version = "1.0.2";

  src = fetchurl {
    url = "https://github.com/robocode-dev/tank-royale/releases/download/v${finalAttrs.version}/robocode-tankroyale-gui-${finalAttrs.version}.jar";
    sha256 = "sha256-9p33waOke++m0Rv3H2D6p6FFK5js8KQXwMFqwIZOarI=";
  };

  dontUnpack = true;

  nativeBuildInputs = [ makeWrapper ];

  installPhase = ''
    runHook preInstall

    install -Dm444 $src $out/share/java/robocode-tankroyale-gui.jar

    makeWrapper ${jdk}/bin/java $out/bin/robocode \
      --add-flags "-jar $out/share/java/robocode-tankroyale-gui.jar"

    runHook postInstall
  '';

  meta = with lib; {
    description = "Robocode Tank Royale — programming game where you code robot tanks to battle";
    homepage = "https://robocode.dev/";
    license = licenses.epl20;
    mainProgram = "robocode";
    platforms = platforms.linux;
  };
})
