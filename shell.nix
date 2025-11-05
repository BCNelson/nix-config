# Shell for bootstrapping flake-enabled nix and home-manager
# You can enter it through 'nix develop' or (legacy) 'nix-shell'

{ pkgs ? (import ./nixpkgs.nix) { }, system, lib, ... }:
let
  rustpkg = pkgs.rust-bin.stable.latest.default.override {
    extensions = [ "rust-src" ];
  };
  rustPackages = with pkgs; [
    rustpkg
    openssl
    pkg-config
    cargo-deny
    cargo-edit
    cargo-watch
    rust-analyzer
    cmake
  ];
in
{
  default = pkgs.mkShell {
    # Enable experimental features without having to specify the argument
    NIX_CONFIG = "experimental-features = nix-command flakes";
    JUST_UNSTABLE = "true"; #Must be enabled for just modules to work
    env = {
      # Required by rust-analyzer
      RUST_SRC_PATH = "${rustpkg}/lib/rustlib/src/rust/library";
    };
    nativeBuildInputs = with pkgs; [
      nix
      home-manager
      git
      git-crypt
      gnupg
      pinentry-tty
      just
      qemu
      zstd
      terraform
      nixd
      alejandra
      agenix-rekey
      age-plugin-yubikey
      age-plugin-fido2-hmac
      repomix
      claude-code
      bitwarden-cli
    ] ++ lib.optional (lib.hasInfix system == "linux") [
      pkgs.quickemu
      pkgs.qemu
      (pkgs.writeShellScriptBin "qemu-system-x86_64-uefi" ''
        ${pkgs.qemu}/bin/qemu-system-x86_64 \
          -bios ${pkgs.OVMF.fd}/FV/OVMF.fd \
          "$@"
      '')
    ] ++ rustPackages;
  };
}
