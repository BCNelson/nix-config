{ rustPlatform, pkgs, lib, makeWrapper }:
let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
  runtimeDeps = with pkgs; [
    direnv
  ];
in
rustPlatform.buildRustPackage rec {
  pname = cargoToml.package.name;
  inherit (cargoToml.package) version;
  src = ./.;
  cargoLock = {
    lockFile = ./Cargo.lock;
  };
  buildInputs = [ makeWrapper ];
  postFixup = ''
    wrapProgram $out/bin/${pname} \
      --prefix PATH : ${lib.makeBinPath runtimeDeps}
  '';
}
