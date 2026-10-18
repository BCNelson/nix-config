---
name: diagnose-auto-update
description: Work out why a host has not applied a commit (auto-update failed, retries exhausted, wrong branch, stale host, reboot pending, unreachable). Use when a deploy didn't land, nixcfg wait failed on a host stage, or a Cadence/healthcheck failure arrived.
---

# Diagnose a host that didn't apply a change

Start with the summary; it reads the state file, the service and the boot state
in one ssh round trip and says what it thinks is wrong:

```
nixcfg host-status <host>
```

Then match the diagnosis:

| Symptom | Cause | Fix |
|---|---|---|
| `branch=main` / a feature branch | the checkout isn't on `auto-update`, so it deploys that branch | human: `ssh root@<ssh-host> git -C <path> switch auto-update` |
| `retries_exhausted` (ATTEMPT_COUNT >= MAX_RETRIES for this commit) | the build failed 3 times; the host waits for a *new* commit | read the failure (below), fix forward with a new commit. Only if the failure was transient, hand over `nixcfg human reset-retries <host>` |
| attempt n/3 failed, retrying | a failed attempt; it retries on the next timer/ntfy | wait, but read the error now |
| `git-crypt locked` in the log | the checkout lost its git-crypt unlock | human, on the host |
| `rebase` errors | local commits or a dirty tree in the host checkout | human, on the host |
| `switchInhibitors` / boot fallback, `reboot pending` | the switch needed a reboot (kernel/initrd/systemd) | hosts with `reboot = true` do it themselves within a minute; otherwise human: `nixcfg human reboot <host>` |
| unreachable / ssh hangs on whiskey | Tailscale SSH "check" mode wants a browser re-auth | human: `ssh bcnelson@whiskey.b.nel.family true` |
| host months behind, `systemd-run --verbose` errors | stale host: old auto-update vs new nixos-rebuild-ng | human: build on the host, `switch-to-configuration boot`, reboot |

## Reading the failure

- `nixcfg host-status <host> --log` prints the journal of the last run (by
  invocation id).
- romeo keeps only ~1 day of journal. If it's gone, the last 10 KB of the failed
  run is in Cadence (healthchecks on whiskey) as the failure ping body.
- For anything else use the `ssh` MCP tools (`ssh_exec` for short commands,
  `ssh_start` + `ssh_wait` for long ones). Don't start, stop or restart
  `auto-update.service`: that's human-only, and `stop` doesn't work anyway (the
  ntfy subscriber restarts it within seconds).

## After a fix

Commit the fix (pathspec), hand over the push, and
`nixcfg wait <new-sha> --until host_applied --host <host>`.
