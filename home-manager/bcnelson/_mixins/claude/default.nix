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
    };
  };
}