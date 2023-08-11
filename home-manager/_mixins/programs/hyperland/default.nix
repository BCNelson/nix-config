{ config, ... }:

{
  home.file."${config.xdg.configHome}/hypr" = {
    source = ./config;
    recursive = true;
  };
}
