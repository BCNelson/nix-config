{ lib
, stdenvNoCC
, fetchurl
, makeWrapper
, nodejs
}:
let
  tweetnacl = fetchurl {
    url = "https://registry.npmjs.org/tweetnacl/-/tweetnacl-1.0.3.tgz";
    hash = "sha512-6rt+RN7aOi1nGMyC4Xa5DdYiukl2UWCbcJft7YhxReBGQD7OAM8Pbxw6YMo4r2diNEA8FEmu32YOn9rhaiE5yw==";
  };
in
stdenvNoCC.mkDerivation {
  pname = "happy-auth-notify";
  version = "0.1.0";

  src = ./.;

  nativeBuildInputs = [ makeWrapper ];

  dontConfigure = true;
  dontBuild = true;

  installPhase = ''
    runHook preInstall

    libdir=$out/lib/happy-auth-notify
    mkdir -p $libdir/node_modules/tweetnacl
    tar -xzf ${tweetnacl} -C $libdir/node_modules/tweetnacl --strip-components=1
    install -m 0644 $src/happy-auth-notify.mjs $libdir/happy-auth-notify.mjs

    mkdir -p $out/bin
    makeWrapper ${nodejs}/bin/node $out/bin/happy-auth-notify \
      --add-flags "$libdir/happy-auth-notify.mjs"

    runHook postInstall
  '';

  meta = {
    description = "Pair a workstation with Happy and push the QR URL via ntfy";
    license = lib.licenses.mit;
    mainProgram = "happy-auth-notify";
    platforms = lib.platforms.unix;
  };
}
