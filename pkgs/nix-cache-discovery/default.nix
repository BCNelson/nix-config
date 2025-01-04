{ rustPlatform }:
let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
in
rustPlatform.buildRustPackage {
  pname = cargoToml.package.name;
  inherit (cargoToml.package) version;
  src = ./.;
  cargoLock = {
    lockFile = ./Cargo.lock;
  };
  buildInputs = [];
}
