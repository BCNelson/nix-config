{ pkgs, ... }:
{
  # https://opencode.ai - the herdr integration lives in ../herdr, alongside
  # claude's and codex's, so all three agents report pane state the same way.
  programs.opencode = {
    enable = true;

    # Pulls programs.mcp.servers into opencode.json's `mcp` block, same as
    # claude-code and codex do. See ../mcp.
    enableMcpIntegration = true;

    settings = {
      # Nix owns the binary, so the self-updater could only fail against a
      # read-only store path. Take updates through `nix flake update` instead -
      # same reasoning as herdr's update.version_check.
      autoupdate = false;

      # `shell` backs both the interactive terminal and agent tool calls. Left
      # unset, opencode probes for an OS default like /bin/bash or /bin/zsh -
      # neither of which exists on NixOS. Point at the store path instead.
      shell = "${pkgs.bash}/bin/bash";
    };

    # programs.opencode.tui is deliberately unset: opencode persists the theme
    # and other TUI preferences picked from the command palette back into
    # tui.json, which the home-manager module would write as a read-only store
    # symlink. Nothing here needs enforcing, so leave the file to opencode
    # rather than reaching for config-merge the way ../codex has to.
  };
}
