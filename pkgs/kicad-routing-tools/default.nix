# KiCad Routing Tools: a Rust-accelerated A* autorouter, as a KiCad SWIG
# action plugin. The plugin tree lands in
# $out/share/kicad/scripting/plugins/KiCadRoutingTools; link it into KiCad's
# per-user plugin dir and add `pythonDeps` to kicad's pythonPath (see
# home-manager/bcnelson/sierra.nix).
#
# Not a nixpkgs kicad `addons` entry: kicad's stock data path symlinkJoins the
# addons over a base whose scripting/ is itself a symlink, so addon plugins
# are silently dropped (kicad 10.0.7, nixpkgs 02c087f).
{ lib
, stdenv
, fetchFromGitHub
, rustPlatform
, python3
}:
let
  version = "0.23.7";

  src = fetchFromGitHub {
    owner = "drandyhaas";
    repo = "KiCadRoutingTools";
    rev = "v${version}";
    hash = "sha256-srHm2Bmh4gSgE/Flv40S98J9LGfBW2VWTxIdsDToUMc=";
  };

  # The pyo3 extension. Upstream's installer downloads a prebuilt .so from the
  # GitHub release; building it here avoids patchelf-ing a foreign binary.
  gridRouter = rustPlatform.buildRustPackage {
    pname = "kicad-routing-tools-grid-router";
    inherit version src;
    sourceRoot = "${src.name}/rust_router";
    cargoHash = "sha256-EL/dsEcZncvOXFGBRpvhVumIVlzUZJt2/Ooil3sppeM=";
    # pyo3's build script needs an interpreter to probe, even for abi3.
    nativeBuildInputs = [ python3 ];
    doCheck = false;
  };
in
stdenv.mkDerivation {
  pname = "kicad-routing-tools";
  inherit version src;

  dontBuild = true;

  # Mirror package_pcm.py: ship the tree minus build/release tooling and dev
  # dirs, with the extension under the canonical name so the plugin's
  # startup resolver (root __init__.py) has nothing to copy into the store.
  installPhase = ''
    runHook preInstall
    dest=$out/share/kicad/scripting/plugins/KiCadRoutingTools
    mkdir -p $(dirname $dest)
    cp -r . $dest
    rm -rf $dest/{build_router.py,install_plugin.py,package_pcm.py,update_metadata.py,tests,docs,kicad_files,.github,.gitignore,.gitattributes}
    cp ${gridRouter}/lib/libgrid_router.so $dest/rust_router/grid_router.so
    runHook postInstall
  '';

  passthru = {
    inherit gridRouter;
    # requirements.txt. Upstream's first-run check otherwise offers to
    # pip-install them into KiCad's (read-only) Python.
    pythonDeps = with python3.pkgs; [ numpy scipy shapely pillow ];
  };

  meta = {
    description = "Rust-accelerated A* autorouter plugin for KiCad";
    homepage = "https://github.com/drandyhaas/KiCadRoutingTools";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" ];
  };
}
