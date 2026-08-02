{ pkgs, ... }:
{
  languages.python = {
    enable = true;
    uv = {
      enable = true;
      sync.enable = true;
    };
  };

  dotenv.enable = true;

  enterShell = ''
    echo "Python $(python --version)"
  '';
}
