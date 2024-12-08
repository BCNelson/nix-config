{ rustPlatform, pkgs, lib, makeWrapper }:
let
  cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
  runtimeDeps = with pkgs; [
    git
    gnupg
    git-crypt
    coreutils
    just
    age-plugin-yubikey
    age-plugin-fido2-hmac
    nix
    nixos-install-tools
    openssh
    util-linux
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
      --prefix PATH : ${"/run/wrappers/bin:" + (lib.makeBinPath runtimeDeps)}
  '';
}
