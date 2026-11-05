{ pkgs, ... }:
let
  # opencode 2.2.5's node_modules hash in nixpkgs is stale: the bun install
  # runs ~7 minutes, then fails with a hash mismatch, and since nothing is
  # cached it took the whole system rebuild down with it on every desktop.
  # Fixed on nixpkgs master in c399e1b7fb ("opencode: fix hash"). Rather than
  # carry that fix, leave opencode out until the channel has it: comparing
  # the hash only evaluates, it builds nothing, so the first flake bump that
  # brings in a new hash (that fix or a version bump) turns opencode back on
  # by itself. Drop this guard then.
  staleHash = "sha256-xfOVcw1+waLVNhFDwzOtzxrUSQEogvn88YomydOlhwc=";
in
{
  # https://opencode.ai - the herdr integration lives in ../herdr, alongside
  # claude's and codex's, so all three agents report pane state the same way.
  programs.opencode = {
    enable = pkgs.opencode.node_modules.outputHash != staleHash;

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
      #
      # A bash that sends `-c` (tool calls) through lowprio and leaves the
      # interactive terminal alone. See pkgs/lowprio.
      shell = "${pkgs.lowprio.agentShell}/bin/bash";
    };

    # programs.opencode.tui is deliberately unset: opencode persists the theme
    # and other TUI preferences picked from the command palette back into
    # tui.json, which the home-manager module would write as a read-only store
    # symlink. Nothing here needs enforcing, so leave the file to opencode
    # rather than reaching for config-merge the way ../codex has to.
  };
}
