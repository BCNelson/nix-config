{ pkgs, ... }:
{
  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_22;
    npm.enable = true;
  };

  env.NODE_ENV = "development";

  dotenv.enable = true;

  enterShell = ''
    echo "Node.js $(node --version) with npm $(npm --version)"
  '';
}
