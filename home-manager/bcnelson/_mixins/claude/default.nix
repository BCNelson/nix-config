{pkgs, ...}: {
  programs.claude-code = {
    enable = true;
    enableMcpIntegration = true;
    package = pkgs.claude-code;
    settings = {
      includeCoAuthoredBy = false;
      permissions = {
        defaultMode = "plan";
        disableBypassPermissionsMode = "disable";
      };
      theme = "dark";
      # The fullscreen renderer owns the mouse and copies a selection to the
      # clipboard on release (copyOnSelect, on by default). It writes over
      # OSC 52, which the Herdr desktop app forwards to the system clipboard,
      # so selecting in a Claude pane copies without Ctrl+Shift+C. This file
      # is a read-only store link, so `/tui` inside Claude can't persist it.
      tui = "fullscreen";
      # Bash tool calls run in their own scope under lowprio.slice, so a build
      # the agent starts cannot starve the agent itself. See pkgs/lowprio.
      env.CLAUDE_CODE_SHELL_PREFIX = "${pkgs.lowprio.claudePrefix}/bin/claude-shell-prefix";
    };
  };
}