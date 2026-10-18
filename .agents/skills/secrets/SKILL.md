---
name: secrets
description: Add, edit, rotate or debug agenix-rekey secrets (and understand git-crypt sensitive.nix values) in this NixOS flake. Use whenever a change needs a password, token, key or other secret, or a host fails to evaluate because a rekeyed secret is missing.
---

# Secrets

Decrypting needs the human's hardware key (YubiKey / Solo), so you prepare
everything around that one step and hand it over. You never read plaintext.

## Check the state first

```
nixcfg secrets-status <host>     # or --all
```

For every declared `age.secrets` entry it reports `ok`, `missing_master` (the
`rekeyFile` doesn't exist), `stale_or_missing_rekey` (needs `just rekey`), or
`untracked_rekey` (exists but not `git add`ed, so the flake can't see it), plus
orphaned rekeyed files and who should fix each one.

## Add a generated secret (password, UUID, key pair)

1. Declare it next to the service that uses it:

   ```nix
   age.secrets.myservice_password = {
     rekeyFile = ./secrets/myservice_password.age;
     generator.script = "passphrase";   # or "alnum", "base64", "hex", or a function
     owner = "myservice";
     mode = "0400";
   };
   ```

   Secrets shared by several hosts live under `secrets/store/` (shared ones in
   `secrets/store/shared/`); per-host service secrets live in
   `nixos/<host>/services/secrets/`. Optionally add
   `bitwarden = { name = "..."; uris = [ ... ]; };` to sync it to Bitwarden.
2. `just generate-secrets` -- encrypts to the master keys, **no key touch**.
3. `git add` the new `.age` (and any `.pub` the generator wrote alongside it).
4. Hand the human: `just rekey`
5. `git add secrets/hosts`, then commit everything with a pathspec.

Between steps 1 and 4 the host **will not evaluate** (its rekeyed file doesn't
exist yet), so CI and `just check-host` fail for it. That's expected; don't
"fix" it by removing the declaration.

## Add a hand-entered secret, or edit one

Human-only (it decrypts or prompts for plaintext). Prepare the declaration
(without a `generator`), then hand over, one line:

`agenix edit nixos/<host>/services/secrets/<name>.age && just rekey`

Then `git add` the `.age` and `secrets/hosts`, and commit with a pathspec.

## Rotate a generated secret

Hand the human: `agenix generate -f -t <tag> && just rekey`

## Using a secret in config

- File path at runtime: `config.age.secrets.<name>.path` (under `/run/agenix`).
- Spliced into an env file: `age-template.files.<name> = { vars.token = config.age.secrets.<name>.path; content = "TOKEN=$token"; };`
  then use `config.age-template.files.<name>.path`.
- Never `builtins.readFile` a secret: that copies it into the Nix store.

## git-crypt `sensitive.nix`

`nixos/sensitive.nix`, `nixos/romeo/sensitive.nix`, `nixos/whiskey/sensitive.nix`
hold evaluation-time values read through `libx.getSecret` /
`getSecretWithDefault`. They end up in the world-readable Nix store, and when
the repo is locked (as in CI) they silently fall back to the default. Don't add
new values there; use agenix. Don't read them. If a change needs a new
`sensitive.nix` value, tell the human the attribute name and where it's used.
