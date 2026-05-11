{ config, pkgs, ... }:
let
  happyHomeDir = "${config.xdg.dataHome}/happy";

  happy-coder = pkgs.symlinkJoin {
    name = "happy-coder-wrapped-${pkgs.happy-coder.version}";
    paths = [ pkgs.happy-coder ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      wrapProgram $out/bin/happy \
        --set-default HAPPY_HOME_DIR ${happyHomeDir}
      wrapProgram $out/bin/happy-mcp \
        --set-default HAPPY_HOME_DIR ${happyHomeDir}
    '';
    inherit (pkgs.happy-coder) meta;
  };
in
{
  home.packages = [
    happy-coder
    pkgs.happy-auth-notify
  ];
}
