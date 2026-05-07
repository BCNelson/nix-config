{ lib
, stdenvNoCC
}:

stdenvNoCC.mkDerivation {
  pname = "kwin-adaptive-workspaces";
  version = "0.1.0";

  src = ./.;

  installPhase = ''
    runHook preInstall

    script_dir="$out/share/kwin/scripts/adaptive-workspaces"
    install -Dm644 metadata.json "$script_dir/metadata.json"
    mkdir -p "$script_dir/contents"
    cp -R contents/* "$script_dir/contents/"

    runHook postInstall
  '';

  meta = {
    description = "KWin script that maps virtual desktops to monitors when docking";
    license = lib.licenses.mit;
    platforms = lib.platforms.linux;
  };
}
