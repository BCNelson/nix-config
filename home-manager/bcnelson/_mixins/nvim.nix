{ inputs, ... }:
{
  imports = [
    inputs.nixvim.homeManagerModules.nixvim
  ];

  programs.nixvim = {
    enable = true;
    colorschemes.catppuccin.enable = true;
    plugins = {
      lsp = {
        enable = true;
        servers = {
          tsserver.enable = true;
          lua-ls.enable = true;
          rust-analyzer.enable = true;
        };
      };
      telescope = {
        enable = true;
      };
      treesitter = {
        enable = true;
      };
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
