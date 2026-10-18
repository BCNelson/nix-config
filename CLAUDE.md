# CLAUDE.md

@AGENTS.md

## Claude Code specifics

- Hooks and project permissions are in `.claude/settings.json` (committed); the
  guard runs on Bash, Read, Grep, Glob and the ssh MCP tools. Personal
  allow-lists belong in `.claude/settings.local.json`, and a guard denial wins
  over them.
- `.claude/skills` is a symlink to `.agents/skills`; edit the skills there.
- MCP servers for this repo come from `.mcp.json` (`nixcfg`, `ssh`, `nix`,
  `grafana`, `devenv`). User-wide servers go through the gateway configured in
  `home-manager/bcnelson/_mixins/mcp/` -- never set `programs.claude-code.mcpServers`
  directly for a server Codex should also see.
- For long waits, run `nixcfg wait ... ` with Bash `run_in_background` (you are
  re-invoked when it exits) or under Monitor; don't poll with sleeps.
- Bash commands run under the `lowprio` shell prefix (`lowprio.slice`), so builds
  you start are deliberately low-priority.
