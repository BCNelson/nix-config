# AGENTS.md

Instructions for AI coding agents (Claude Code, Codex, opencode, pi) working in
this repository. `CLAUDE.md` imports this file; edit this one.

This is a multi-host NixOS flake (flake-utils-plus). Changes deploy themselves:
a commit on `main` is checked by CI, advanced to the `auto-update` branch, and
every auto-updating host rebuilds from that branch. Most of what goes wrong for
agents here is not Nix -- it is the deploy pipeline and the secrets. Read those
sections before touching either.

## Golden rules

1. **Commit with an explicit pathspec**: `git commit -m "msg" -- path/a path/b`.
   This tree usually has unrelated staged work in progress; a bare
   `git commit` sweeps it into `main`, and `main` deploys. Never `git commit -a`.
2. **`git add` new files before any `nix` command.** Flakes only see tracked
   files. This includes new `.age` files and new `secrets/hosts/<host>/` dirs.
3. **Never read secrets.** Don't read, grep, cat or `nix eval` any
   `**/sensitive.nix` (git-crypt, and this checkout is usually unlocked), never
   decrypt `.age` files, never read `/run/agenix*` on hosts.
4. **Some steps belong to the human.** Anything that needs a hardware key
   touch, kicking or resetting auto-update, and activating a system
   (`nixos-rebuild switch`, `just update-os`) are human-only. Hand the human the
   exact command **on one line** -- no backslash continuations. Pushing to
   `main` is yours (`git push origin main`): CI gates the deploy. Never
   force-push, and never push `auto-update` -- only CI moves it.
5. **Write Nix `${...}` interpolations with your file-edit tool,** not shell
   heredocs: the shell tool expands `${...}` even inside quoted heredocs.
6. **Use `nixcfg wait` to wait for anything** (CI, auto-update, a host applying a
   change, a build). Don't hand-roll `sleep` loops. See "Waiting".

A guard hook enforces 1, 3 and 4 for every agent (see "Guardrails"). If it
blocks you, it tells you what to hand the human instead; do that rather than
looking for another way round.

## Repo map

| Path | What |
|---|---|
| `flake.nix` | inputs, `hosts` (one `libx.mkHost` per host; `# INSERT_NEW_HOST_CONFIG_HERE`), `systemConfigs` (redo-3, system-manager), `homeConfigurations` |
| `lib/default.nix` | `mkHost`, `mkHome`, `getSecret`/`getSecretWithDefault` (git-crypt helpers) |
| `nixos/` | `default.nix` splits the hostname on `-`: `romeo-2` imports `nixos/romeo/` and `nixos/romeo/2.hardware-configuration.nix` |
| `nixos/_mixins/` | shared NixOS config: `roles/`, `hardware/`, `users/` |
| `modules/nixos/` | custom modules: `autoUpdate`, `generationRetention`, `thinClientBuilder`, `bitwarden-sync`, ... |
| `home-manager/` | user environments; `bcnelson/_mixins/` holds agent, MCP and tool config |
| `pkgs/` | in-house packages: `pkgs/<name>/` + an entry in `pkgs/default.nix`; Go MCP servers (`ssh-mcp`, `nixcfg-mcp`), Rust CLIs |
| `secrets/` | agenix-rekey: `store/` master-encrypted sources, `hosts/<host>/` rekeyed outputs, `masterKeys/` identity stubs |
| `hosts/data/<host>.nix` | each host's SSH `hostKey` (used for rekeying). Not a host registry |
| `.forgejo/` | CI: `workflows/check.yml` (deploy gate), `update.yml` (daily flake bump), `scripts/` |
| `.agents/` | agent tooling data: `hosts.json` (ssh destinations), `skills/` |
| `main.tf` | OpenTofu (DNS); `just tofu`, never `terraform` |

List hosts: `nix eval .#nixosConfigurations --apply 'c: builtins.concatStringsSep "\n" (builtins.attrNames c)' --raw`
(and `.#systemConfigs` for redo-3). Flake names carry a numeric suffix
(`romeo-2`, `whiskey-1`); ssh names don't (`romeo.b.nel.family`).

## Commands

| Command | What it actually does | Who |
|---|---|---|
| `just check-host <host>` | `nix build .#nixosConfigurations.<host>.config.system.build.toplevel --dry-run` (evaluate + plan) | agent |
| `just build-host <host>` | real build of a host's toplevel, no activation | agent |
| `just fmt` | `nix fmt` = deadnix + statix (Alejandra is disabled in the formatter) | agent |
| `just update` | `nix flake update` | agent (commit `flake.lock` with a pathspec) |
| `just generate-secrets` | `agenix generate -a`; encrypts newly declared generated secrets to the master keys. No key touch | agent |
| `just rekey` | `agenix rekey -a`; re-encrypts master secrets for every host into `secrets/hosts/`. **Key touch** (it does not generate keys) | human |
| `just new-secrets` | generate + rekey. **Key touch** | human |
| `just unlock` / `lock` | git-crypt via GPG. **Key touch** | human |
| `git push origin main` | push; CI checks it and, if green, deploys it | agent |
| `just push` / `pull` / `sync` | git with a token read from `sensitive.nix` (use plain `git push`) | human |
| `just tofu ...` | OpenTofu with B2 credentials read from `sensitive.nix` | human |
| `just update-os` (`apply`, `os`, `o`, `all`) | `nixos-rebuild switch` on *this* machine (`all` is OS only, not home) | human |
| `just update-home` | `home-manager switch` | human |
| `just status`, `just wait <sha> ...` | deploy status and waiting, via `nixcfg` (below) | agent |

`just build` / `just test` default to `vm_test`, which is not a flake host;
always pass a real machine.

## How deploys work

1. A commit lands on `main`. **CI only runs if it touches a watched path**
   (`.forgejo/workflows/check.yml`, `on.push.paths`): `**.nix`, `**.age`,
   `nixos/**/*.{yaml,yml,py,cjs}`, `flake.lock`, `.forgejo/scripts/**`,
   `check.yml` itself. Anything else -- including Go/Rust sources under `pkgs/`
   -- never deploys on its own; it rides along with the next watched change or
   the daily flake bump (`update.yml`, 09:00 UTC). `nixcfg explain` tells you.
2. `check-hosts` **evaluates** every host (`--dry-run`). It does not build, so a
   build failure first shows up on the hosts themselves.
3. If that passes, CI advances `auto-update` to the checked commit --
   **forward only** (`.forgejo/scripts/advance-auto-update.sh`): if a newer
   commit has already shipped, an older run is a no-op -- and pings ntfy.
4. Hosts with an ntfy subscriber start `auto-update.service` within seconds; the
   rest wait for their timer. Each host fetches (Forgejo, GitHub fallback),
   rebases **whatever branch its checkout has checked out** (normally
   `auto-update`), and runs `nixos-rebuild switch` locally.
5. Retry state lives in `/var/lib/auto-update/state` (`BUILT_COMMIT`,
   `ATTEMPT_COMMIT`, `ATTEMPT_COUNT`). A commit gets `MAX_RETRIES` (3) attempts,
   then the host stops building it and keeps reporting failure until a *new*
   commit arrives. If a reboot is required and the host has `reboot = true`, it
   schedules `shutdown -r +1`.

Gotchas:

- `systemctl stop auto-update.service` doesn't pause a deploy: the ntfy
  subscriber restarts it within seconds.
- A host on the wrong branch, or one that has sat stale for months, can fail in
  odd ways (old auto-update vs new nixos-rebuild). See the `diagnose-auto-update`
  skill.
- romeo keeps only ~1 day of journal. A "clean" boot from longer ago means the
  logs are gone, not that it was clean. Cadence (healthchecks on whiskey) keeps
  the last 10 KB of each failed run.
- Old generations are pruned by `services.bcnelson.generationRetention`, not
  `nix.gc.options`.

| Host | Observable | Checkout | Interval | ntfy | Reboots |
|---|---|---|---|---|---|
| romeo-2 | yes | /config | 1h | yes | yes |
| whiskey-1 | yes | /config | 1h | yes | yes |
| vor-2 | yes | /config | 1h | yes | yes |
| charlie-1 | yes | /config | 1h | yes | yes |
| ryuu-2 | yes | /config | 5m | no | yes |
| sierra-2, golf-* | no | ~bcnelson/nix-config | 6h | yes | no |
| bravo-1 | no | /config | 6h | yes | no |
| xray-* | no | /home/config (xray-3: /config) | 1h | no | no |
| qilin-1, berg-1 | no | /config | 24h | no | berg only |

"Observable" means `nixcfg` can ssh in and check whether a change applied
(`.agents/hosts.json`). wyse-1 is a thin client fed by romeo's
`thin-client-build`.

## Before you hand over a change

1. `just check-host <host>` for every host the change touches (or
   `nixcfg check-host --all`).
2. Build anything expensive for real: `just build-host <host>`.
3. **Pre-seed romeo** for heavy changes that romeo-2 or whiskey-1 will build
   (whiskey offloads its builds to romeo and can't build V8 itself):
   `nixcfg preseed romeo-2`. Otherwise the auto-update rebuilds it from scratch.
   Pre-seeding needs a clean checkout at the commit you'll push.
4. Commit with a pathspec, then `git push origin main`.
5. `nixcfg wait <sha> --until live` and report the summary it prints.

## Secrets

Two separate systems.

**agenix-rekey (runtime secrets, the default).** A module declares
`age.secrets.<name>.rekeyFile = ./secrets/<name>.age;` (optionally
`generator.script`, `owner`, `mode`, `bitwarden = {...}`, or an
`age-template.files` entry to splice it into an env file). The master file is
encrypted to the YubiKey / Solo key; `just rekey` re-encrypts it for each host
into `secrets/hosts/<host>/<hash>-<name>.age`, which the host decrypts at
activation into `/run/agenix`.

- **New generated secret** (agent does most of it): declare it with a
  `generator`, run `just generate-secrets` (no touch), `git add` the new `.age`
  (and any `.pub` the generator writes), then hand the human `just rekey`, then
  `git add secrets/hosts` and commit everything with a pathspec.
- **New hand-entered secret or edit**: human-only. Hand them
  `agenix edit <path>.age` followed by `just rekey`; you then `git add` and commit.
- **Rotate a generated secret**: human: `agenix generate -f -t <tag>` then
  `just rekey`.
- A host **does not evaluate** between declaring a secret and rekeying it (the
  rekeyed file is missing). `nixcfg secrets-status <host>` shows what is missing
  or stale and who needs to do what, without decrypting anything.

**git-crypt (`**/sensitive.nix`, evaluation-time values).** Read through
`libx.getSecret` / `getSecretWithDefault`. When the repo is locked (as in CI),
these silently return the default, so CI cannot catch a missing value. The
values end up in the world-readable Nix store, so prefer agenix for anything new.
Never read these files.

## Waiting

`nixcfg wait` treats a deploy as one timeline per commit and waits for the
milestone you name, failing fast with the stage, the reason and a log tail if
anything before it goes wrong:

```
pushed -> ci_started -> check_hosts_passed -> ci_passed -> auto_update_advanced
  -> host_fetched -> host_building -> host_applied -> host_rebooted_ok -> live
```

- `nixcfg wait <sha> --until ci_passed`
- `nixcfg wait <sha> --until live` (default host set: observable hosts that will
  apply soon; it prints which hosts it chose and why others were left out)
- `nixcfg wait <sha> --until host_applied --host romeo-2 --host whiskey-1`
- `nixcfg wait job <id>` for a background build started by `check-host`/`preseed`

It prints one line per event and ends with `RESULT state=...`. Exit codes:
0 succeeded, 1 failed, 2 usage error, 3 timed out, 4 unreachable,
5 will not happen (e.g. the commit touches no watched path), 6 blocked on the
human (e.g. Tailscale SSH wants a browser login, a host needs a kick). A commit
counts as applied once a host has built it *or any descendant*.

Run it in the background (Claude: background Bash or Monitor) for long waits.
Over MCP, `wait_for` blocks for a few minutes at a time and returns
`state: pending` with a `wait_id`; call it again with the same `wait_id` until
it finishes. Waits survive the MCP server restarting.

## nixcfg

One binary, `nixcfg` (CLI) / `nixcfg-mcp serve` (MCP, configured in `.mcp.json`
and per-agent configs), in `pkgs/nixcfg-mcp`. It's in the dev shell; outside it,
`nix run .#nixcfg-mcp -- <cmd>`.

| CLI | MCP tool | Use it to |
|---|---|---|
| `nixcfg status [sha]` | `deploy_status` | see where a commit is and why it hasn't deployed |
| `nixcfg wait ...` | `wait_for`, `wait_status`, `wait_cancel` | wait for a milestone (above) |
| `nixcfg hosts` | `hosts_list` | list hosts, their auto-update settings, observability |
| `nixcfg host-status <host>` | `host_status` | a host's branch, state file, service, pending reboot, plus a diagnosis |
| `nixcfg explain [rev\|range]` | `explain_change` | whether a change will trigger CI, and why |
| `nixcfg check-host <host> [--build]` | `check_host` | evaluate or build a host as a background job |
| `nixcfg preseed <host>` | `preseed` | build romeo-2/whiskey-1 locally and copy to romeo |
| `nixcfg jobs`, `nixcfg wait job <id>` | `jobs_list`, `job_status`, `job_wait`, `job_cancel` | follow background jobs |
| `nixcfg secrets-status <host>` | `secrets_status` | declared secrets vs rekeyed outputs, and the fix for each |
| `nixcfg human <action>` | `human_command` | the exact one-line command to hand the human |

For anything else on a host (logs, systemctl), use the `ssh` MCP server
(`ssh_exec`, `ssh_start`/`ssh_wait` for long jobs). Non-interactive ssh to
whiskey can hang on a Tailscale SSH "check" re-auth; that is a login prompt, not
a dead host -- hand the human `ssh bcnelson@whiskey.b.nel.family true`.

## Guardrails

`scripts/agent-guard` (→ `nixcfg guard`) runs before shell and file-read tool
calls in every agent (`.claude/settings.json`, `.codex/hooks.json`,
`.opencode/plugins/`, `.pi/extensions/`). It blocks:

- `git commit` without an explicit pathspec, or with `-a`/`--interactive`/`--patch`
- reading `**/sensitive.nix`, decrypting `.age`, reading `/run/agenix`, git-crypt
  unlock/export, `gpg -d`, `bw get`
- key-touch commands: `agenix rekey|edit`, `just rekey|new-secrets|unlock|push|tofu|...`
- force pushes (`-f`, `--force-with-lease`, `+refspec`, `--delete`) and any push to `auto-update`
- deploy controls: ntfy pings, `systemctl start auto-update`, editing the state file
- activation: `nixos-rebuild switch|boot|test`, `just update-os` and its aliases

Each block says why and gives the one-liner to hand the human.

## Code style

- Format with `just fmt`. 2-space indent; camelCase attributes, snake_case files.
- Module shape `{ config, pkgs, lib, ... }: { ... }`; shared config goes in a
  `_mixins/` directory.
- Comments explain *why*, at length when the reason is non-obvious; match the
  surrounding density.
- Conventional commits with a scope: `feat(romeo): ...`, `fix(tailscale): ...`,
  `ci(forgejo): ...`.

## Patching nixpkgs

When a nixpkgs PR is slow to merge, add `./patches/<pr>.patch` to
`channels.nixpkgs-unstable-small-patched.patches` in `flake.nix` (get hashes with
`nix-prefetch-url`). If it conflicts because unstable lags master, wait a few
days or temporarily add the PR branch as an input and use it as a host's
`channelName`. Remove the patch once the PR lands.

## Skills

`.agents/skills/` (also `.claude/skills/`):

- `deploy-change`: from edit to verified-live
- `secrets`: adding, editing, rotating agenix secrets
- `add-host`: bringing a new machine into the flake
- `diagnose-auto-update`: a host didn't apply a change
