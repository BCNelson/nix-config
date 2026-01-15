{pkgs, ...}: {
  programs.claude-code = {
    skills = {
      prReviewResponse = ./src;
    };
  };
}
