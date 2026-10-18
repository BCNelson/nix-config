---
name: add-host
description: Bring a new machine into this NixOS flake (host data, flake entry, nixos config, secrets, auto-update, agent observability). Use when asked to add, provision or onboard a host.
---

# Add a host

Host names are `<prefix>-<n>`: `nixos/<prefix>/` is shared by every `<prefix>-*`
machine and `nixos/<prefix>/<n>.hardware-configuration.nix` is per-machine.

1. **Host key.** `hosts/data/<host>.nix` = `{ hostKey = "ssh-ed25519 AAAA..."; }`
   (the machine's `/etc/ssh/ssh_host_ed25519_key.pub`; the human can get it).
2. **Flake entry.** In `flake.nix`, at `# INSERT_NEW_HOST_CONFIG_HERE`:
   `"<host>" = libx.mkHost { hostname = "<host>"; usernames = [ "bcnelson" ]; };`
   (add `desktop = "kde6";` for a desktop).
3. **Config.** `nixos/<prefix>/default.nix` (if new) and
   `nixos/<prefix>/<n>.hardware-configuration.nix`.
4. **Secrets dir.** `mkdir -p secrets/hosts/<host> && touch secrets/hosts/<host>/.gitkeep`
   and `git add` it -- agenix-rekey reads through git and the host won't evaluate
   without it. Then hand the human `just rekey` so the host gets its secrets.
5. **Auto-update.** `services.bcnelson.autoUpdate = { enable = true; path = "/config"; ... };`
   (see an existing host such as `nixos/vor/default.nix`), plus a Cadence check in
   `nixos/whiskey/services/cadence.nix` if it should be monitored.
6. **Make it observable to agents.** Add it to `.agents/hosts.json` with its ssh
   destination, and to the `ssh` server's `--host` list in `.mcp.json`.
7. `git add` everything new, `just check-host <host>`, commit with a pathspec.

Installation itself (`install-system`, ISO) is done by the human.
