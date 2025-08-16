{ rustPlatform, pkgs, lib, makeWrapper, pkg-config, openssl }:
let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
  runtimeDeps = with pkgs; [
    bitwarden-cli
    age
    age-plugin-yubikey
    age-plugin-fido2-hmac
    nix
    jq
  ];
in
rustPlatform.buildRustPackage rec {
  pname = cargoToml.package.name;
  inherit (cargoToml.package) version;
  src = ./.;
  cargoLock = {
    lockFile = ./Cargo.lock;
  };
  
  nativeBuildInputs = [ pkg-config makeWrapper ];
  buildInputs = [ openssl ];
  
  postFixup = ''
    wrapProgram $out/bin/${pname} \
      --prefix PATH : ${lib.makeBinPath runtimeDeps}
  '';
  meta = with lib; {
    description = "Sync age-encrypted secrets to Bitwarden";
    license = licenses.mit;
    maintainers = [ ];
  };
}