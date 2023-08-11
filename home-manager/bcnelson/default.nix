{ outputs, hostname, desktop, ... }:

let
    # Get the hostname prefix from the hostname (e.g. sierria in sierria-1)
    hostnamePrefix = builtins.substring 0 (builtins.indexOf "-" hostname);
in
{
  imports = []
    ++ lib.optional (builtins.isPath ./${hostnamePrefix}.nix) ./${hostnamePrefix}.nix
    ++ lib.optional (builtins.isString desktop) ./desktop.nix ;
  
  home.username = "bcnelson";
  home.homeDirectory = "/home/bcnelson";

  programs = {
    fish = {
      enable = true;
      interactiveShellInit = ''
        set fish_greeting # Disable greeting
      '';
      plugins = [
        {
          name = "done";
          inherit (pkgs.fishPlugins.done) src;
        }
        {
          name = "z";
          inherit (pkgs.fishPlugins.z) src;
        }
      ];
    };
    bash.enable = true;
    git = {
      enable = true;
      userName = "Bradley Nelson";
      userEmail = "bradely@nel.family";
    };
  };

  home.sessionVariables = {
    EDITOR = "vim";
    SSH_AUTH_SOCK = "/run/user/1000/gnupg/S.gpg-agent.ssh"; # Todo remove this when it should not be needed but it is.
    SSH_AGENT_PID = "";
  };
}
