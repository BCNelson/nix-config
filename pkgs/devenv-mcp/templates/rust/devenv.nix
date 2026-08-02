{ pkgs, ... }:
{
  languages.rust = {
    enable = true;
    components = [ "rustc" "cargo" "clippy" "rustfmt" "rust-analyzer" "rust-src" ];
  };

  packages = with pkgs; [
    pkg-config
    openssl
    git
  ];

  enterShell = ''
    echo "Rust $(rustc --version)"
  '';
}
