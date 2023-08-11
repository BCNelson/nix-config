{ outputs, username, stateVersion, lib, ... }:

{
  imports = [] ++ lib.optional (builtins.isPath ./${username}) ./${username};
  home.stateVersion = stateVersion;
  programs.home-manager.enable = true;
}
