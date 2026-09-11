{
  lib,
  stdenv,
  buildVscode,
  fetchurl,
  appimageTools,
  commandLineArgs ? "",
}:
# Cursor is closed source: upstream ships a prebuilt AppImage and nothing else,
# so "building it ourselves" means unpacking that AppImage and running it
# through the same vscode-generic builder nixpkgs uses. This is a trimmed copy
# of nixpkgs' pkgs/by-name/co/code-cursor/package.nix; the difference that
# matters is ./sources.json, which we bump on our own schedule with
# ./update.sh, because the nixpkgs copy routinely trails Cursor's stable
# channel by several releases.
#
# ../../overlays/default.nix compares this against the channel's code-cursor
# and takes whichever is newer, so this derivation drops out of the build by
# itself once nixpkgs catches up.
#
# Darwin is deliberately not carried here (this config has no darwin hosts).
# meta.platforms below is what the overlay reads to decide whether we can serve
# a given system at all, so leaving darwin out keeps it on the nixpkgs build.
let
  sourcesJson = lib.importJSON ./sources.json;
  inherit (sourcesJson) version;

  source =
    sourcesJson.sources.${stdenv.hostPlatform.system}
    or (throw "cursor: ./sources.json has no ${stdenv.hostPlatform.system} source; nixpkgs' code-cursor covers the rest");
in
  (buildVscode {
    inherit commandLineArgs version;

    # Only used on darwin, where we defer to nixpkgs anyway.
    useVSCodeRipgrep = false;

    # Cursor reports vscode >= 1.122 but still ships @vscode/ripgrep. Capping
    # the build-time version keeps vscode-generic from rewriting the bundle.
    vscodeVersion =
      if lib.versionAtLeast sourcesJson.vscodeVersion "1.122.0"
      then "1.121.0"
      else sourcesJson.vscodeVersion;

    pname = "cursor";
    executableName = "cursor";
    longName = "Cursor";
    shortName = "cursor";
    libraryName = "cursor";
    iconName = "cursor";

    src = appimageTools.extract {
      pname = "cursor";
      inherit version;
      src = fetchurl {inherit (source) url hash;};
    };
    sourceRoot = "cursor-${version}-extracted/usr/share/cursor";

    tests = {};
    updateScript = ./update.sh;

    # Cursor ships a launcher script that resolves its own VSCODE_PATH.
    patchVSCodePath = false;

    meta = {
      description = "AI-powered code editor built on vscode";
      homepage = "https://cursor.com";
      changelog = "https://cursor.com/changelog";
      license = lib.licenses.unfree;
      sourceProvenance = [lib.sourceTypes.binaryNativeCode];
      platforms = ["x86_64-linux" "aarch64-linux"];
      mainProgram = "cursor";
    };
  })
  .overrideAttrs (oldAttrs: {
    # musl-based node modules ship in the AppImage but are not used on glibc.
    autoPatchelfIgnoreMissingDeps =
      (oldAttrs.autoPatchelfIgnoreMissingDeps or [])
      ++ ["libc.musl-*.so.*"];

    preFixup =
      (oldAttrs.preFixup or "")
      + ''
        sed -i '/^Keywords=/a MimeType=application/x-cursor-workspace;' \
          $out/share/applications/cursor.desktop
      '';

    postInstall =
      (oldAttrs.postInstall or "")
      + ''
        install -Dm644 ../mime/packages/cursor-workspace.xml -t $out/share/mime/packages
        # The bundled updater cannot rewrite a read-only store path, so Cursor
        # would nag about an update it can never apply. The flake is the only
        # thing that moves this package.
        rm -f $out/lib/cursor/resources/appimageupdatetool.AppImage
      '';
  })
