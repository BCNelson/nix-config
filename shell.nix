# Shell for bootstrapping flake-enabled nix and home-manager
# You can enter it through 'nix develop' or (legacy) 'nix-shell'

{ pkgs ? (import ./nixpkgs.nix) { }, system, lib, ... }: {
  default = pkgs.mkShell {
    # Enable experimental features without having to specify the argument
    NIX_CONFIG = "experimental-features = nix-command flakes";
    JUST_UNSTABLE = "true"; #Must be enabled for just modules to work
    nativeBuildInputs = with pkgs; [
      nix
      home-manager
      git
      git-crypt
      gnupg
      pinentry
      just
      qemu
      zstd
      terraform
      nixd
      nil
      agenix-rekey
      age-plugin-yubikey
      age-plugin-fido2-hmac
    ] ++ lib.optional (lib.hasInfix system == "linux") [
      pkgs.quickemu
      pkgs.qemu
    ];
  };
}
