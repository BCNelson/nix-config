{ inputs, pkgs, ... }:
{
  imports = [
    inputs.nixvim.homeModules.nixvim
  ];

  programs.nixvim = {
    enable = true;
    nixpkgs.source = inputs.nixpkgs-unstable;
    # Suppress while nixos-unstable is still on 26.05 and nixvim master is ahead.
    version.enableNixpkgsReleaseCheck = pkgs.lib.trivial.release != "26.05";
    plugins = {
      lsp = {
        enable = true;
        servers = {
          ts_ls.enable = true;
          lua_ls.enable = true;
          rust_analyzer = {
            enable = true;
            installCargo = true;
            installRustc = true;
          };
        };
      };
      telescope = {
        enable = true;
      };
      treesitter = {
        enable = true;
      };
      lazygit = {
        enable = true;
      };
      neo-tree = {
        enable = true;
      };
      web-devicons.enable = true;
    };

    performance = {
      combinePlugins = {
        enable = true;
      };
      byteCompileLua = {
        enable = true;
      };
    };

    clipboard.providers.wl-copy.enable = true;
  };
}
