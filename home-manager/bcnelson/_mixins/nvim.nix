{ inputs, ... }:
{
  imports = [
    inputs.nixvim.homeManagerModules.nixvim
  ];

  programs.nixvim = {
    enable = true;
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
