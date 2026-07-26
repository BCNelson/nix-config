{ lib, pkgs, ... }: {
  # nixpkgs wraps this with a pinned chromium via PLAYWRIGHT_BROWSERS_PATH, so
  # it needs neither a runtime npm fetch nor `node` on PATH.
  #
  # `npx -y @playwright/mcp@latest` could not work here: npx itself resolves
  # (its shebang is an absolute store path) but the package bin it execs carries
  # `#!/usr/bin/env node`, and nodejs is on nobody's PATH - not the login shell,
  # and not the agents either, since claude-code's wrapper prefixes coreutils
  # and friends but no node. opencode is just the first client to say so out
  # loud: `server unavailable key=playwright type=local status=failed`.
  programs.mcp.servers.playwright.command = lib.getExe pkgs.playwright-mcp;
}
