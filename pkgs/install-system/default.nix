{ rustPlatform }: let
    cargoToml = builtins.fromTOML ./Cargo.toml;
in rustPlatform.buildRustPackage{
  name = cargoToml.package.name;
    version = cargoToml.package.version;
  src = ./.;
  cargoLock = {
    lockFile = ./Cargo.lock;
  };
}