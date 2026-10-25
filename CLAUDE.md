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
- Run long waits (`nixcfg wait ...`, `nixcfg wait job <id>`) under **Monitor**,
  unpiped, with `timeout_ms: 1800000`: each event line reaches the human as it
  happens and wakes you, so you can act on a failure before the wait ends.
  Monitor stops at 30 minutes; when it expires mid-wait, re-arm it with
  `nixcfg wait --id <wait_id>` (the first line printed). Bash `run_in_background`
  is fine for a short wait whose outcome is all you need. Never `| tail`, and
  don't poll with sleeps.
- The status line (`nixcfg statusline`, set in `.claude/settings.json`) shows
  the human every commit on origin/main still in the deploy pipeline (up to 3,
  skipping commits that don't deploy), or the newest deployed one when
  nothing is moving, every 5s. It reads local refs and state files only. The
  cache is updated by any running `--until live` wait on every poll (~15s),
  or otherwise by a detached `nixcfg statusline --refresh` when origin/main
  moves or a commit is still deploying (every 15s); fetches happen at most
  every 30 minutes.
- Bash commands run under the `lowprio` shell prefix (`lowprio.slice`), so builds
  you start are deliberately low-priority.
