{pkgs, ...}: {
  programs.claude-code = {
    enable = true;
    package = pkgs.claude-code-bin;
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