{ config, pkgs, ... }:
let
  happyHomeDir = "${config.xdg.dataHome}/happy";

  # happy-coder 1.1.x discovers claude via PATH but rejects anything whose
  # resolved path lacks a .js/.cjs/.exe extension — treats Nix's wrapped
  # `claude` binary as an npm shell shim and bails. Use the
  # HAPPY_CLAUDE_PATH override (priority-1 in scripts/claude_version_utils.cjs)
  # to point straight at the binary. bun stays on PATH to silence the
  # `which bun` fallback chatter.
  happy-coder = pkgs.symlinkJoin {
    name = "happy-coder-wrapped-${pkgs.happy-coder.version}";
    paths = [ pkgs.happy-coder ];
    nativeBuildInputs = [ pkgs.makeWrapper ];
    postBuild = ''
      wrapProgram $out/bin/happy \
        --set-default HAPPY_HOME_DIR ${happyHomeDir} \
        --set-default HAPPY_CLAUDE_PATH ${pkgs.claude-code}/bin/claude \
        --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.bun ]}
      wrapProgram $out/bin/happy-mcp \
        --set-default HAPPY_HOME_DIR ${happyHomeDir} \
        --set-default HAPPY_CLAUDE_PATH ${pkgs.claude-code}/bin/claude \
        --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.bun ]}
    '';
    inherit (pkgs.happy-coder) meta;
  };
in
{
  home.packages = [
    happy-coder
    pkgs.happy-auth-notify
  ];
}
