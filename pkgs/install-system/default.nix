{ rustPlatform, pkgs }:
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
  buildInputs = with pkgs; [
    git
    gnupg
    git-crypt
    coreutils
    age-plugin-yubikey
    age-plugin-fido2-hmac
  ];
}
