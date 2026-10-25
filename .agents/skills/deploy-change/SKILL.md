---
name: deploy-change
description: Take a change in this NixOS flake from edit to verified-live on the hosts. Use when a change needs to reach machines, when asked to "deploy", "ship", "roll out", or to confirm a change is live.
---

# Deploy a change

Deploys are automatic once a commit reaches `main` -- your job is to make sure
the commit is sound before you push it, and to confirm it landed afterwards.
You push `main` yourself; you never force-push, push `auto-update`, or
activate anything.

## 1. Know what will happen

```
nixcfg explain            # uncommitted + HEAD vs origin/main
nixcfg explain <rev>      # a specific commit or range
```

It says whether the change touches a watched path (otherwise CI never runs and
nothing deploys until the next watched change or the 09:00 UTC flake bump), and
which patterns matched. If nothing is watched and the change must ship now, tell
the human; don't invent a dummy `.nix` edit.

## 2. Check the hosts it affects

- `just check-host <host>` (fast: evaluate + plan), or
  `nixcfg check-host <host>` to run it as a background job and
  `nixcfg wait job <id>`.
- If unsure which hosts are affected, check them all: `nixcfg check-host --all`.
- For anything that changes packages or builds from source, do a real build:
  `nixcfg check-host <host> --build`. CI only evaluates.

## 3. Pre-seed romeo for heavy builds

If romeo-2 or whiskey-1 will have to build something expensive (kernel, a
browser, nodejs/V8, a big Rust or Go package), copy it to romeo first so the
auto-update doesn't rebuild it (whiskey offloads to romeo and cannot build V8):

```
nixcfg preseed romeo-2
nixcfg preseed whiskey-1
```

Pre-seeding needs git-crypt unlocked and a clean checkout at exactly the commit
you'll push, otherwise the derivation won't match what the host builds. So
commit first (step 4), then pre-seed, then push.

## 4. Commit with a pathspec

```
git add <new files>
git commit -m "feat(scope): ..." -- <every path you changed>
```

Never a bare `git commit`: other staged work in this tree would go to `main`.

## 5. Push

`git push origin main`. If it's rejected as non-fast-forward, `git pull --rebase`
and push again; never force.

## 6. Wait for it to be live

```
nixcfg wait <sha> --until live
```

Run it in the background, unpiped (Claude: under Monitor, re-armed with
`nixcfg wait --id <wait_id>` if it expires; never `| tail`, which hides the
progress from the human). It walks CI, the auto-update branch and each host,
and stops at the first real failure with the stage and a log tail. Report the
final summary (per-stage times, per-host result). If it ends:

- `will_not_happen` (5): read the reason -- usually no watched path, a host
  that isn't auto-updating, or `reboot = false` with a reboot needed.
- `blocked` (6): something needs the human (Tailscale SSH login, a host kick). Hand
  them the command it printed.
- `failed` (1): CI or a host build failed. Fix forward with a new commit; for a
  host with retries exhausted, see the `diagnose-auto-update` skill.
- `partial` (7): every host that could be reached applied it; the summary names
  the ones that were unreachable or still going at the deadline. Report them;
  for a host still building, `nixcfg wait <sha> --until live --host <host>`
  again with a longer `--timeout`.
- `unreachable` (4) / `timed_out` (3): `nixcfg host-status <host>` and report.
