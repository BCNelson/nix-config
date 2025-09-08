{pkgs, ...}: {
  programs.claude-code = {
    enable = true;
    package = pkgs.claude-code.overrideAttrs (_oldAttrs: {
      postInstall = ''
        wrapProgram $out/bin/claude \
          --set DISABLE_AUTOUPDATER 1 \
          --prefix PATH ${pkgs.lib.makeBinPath [
            pkgs.coreutils-full
            pkgs.findutils
            pkgs.gnumake
            pkgs.gnused
            pkgs.gnugrep
            pkgs.bash
            pkgs.ripgrep
          ]}
      '';
    });
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