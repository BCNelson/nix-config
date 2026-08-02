{ pkgs, ... }:
{
  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_22;
    corepack.enable = true;
  };

  env.NODE_ENV = "development";

  dotenv.enable = true;

  packages = with pkgs; [
    python3
    gnumake
    gcc
  ];

  enterShell = ''
    echo "Node.js $(node --version) with yarn $(yarn --version)"
  '';
}
