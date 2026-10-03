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
      # Bash tool calls run in their own scope under lowprio.slice, so a build
      # the agent starts cannot starve the agent itself. See pkgs/lowprio.
      env.CLAUDE_CODE_SHELL_PREFIX = "${pkgs.lowprio.claudePrefix}/bin/claude-shell-prefix";
    };
  };
}