{pkgs, ...}: {
  programs.claude-code = {
    skills = {
      initDevenv = ./src;
    };
  };
}
