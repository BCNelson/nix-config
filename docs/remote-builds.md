# Remote builds

Whiskey is a 4-core/4 GB VPS. It can evaluate its own configuration and switch
to it, but compiling on it is hours of thrashing at best and the OOM killer at
worst — nodejs/V8 is the recurring offender, and it is why
`whiskey cannot build V8` keeps coming up.

So it doesn't compile. Whiskey still pulls the repo, evaluates its own closure
and runs its own `nixos-rebuild`; it just hands every derivation to romeo (32
cores, 94 GB) and copies the finished outputs back.

This is *not* the thin-client mechanism ([thin-clients.md](thin-clients.md)).
A thin client cannot even evaluate, so romeo builds a whole system on its behalf
and publishes it to the binary cache. Whiskey drives its own rebuild and only
borrows a compiler.

## Moving parts

| Piece | Where | What it does |
| --- | --- | --- |
| `nixos/whiskey/remote-builds.nix` | whiskey | `nix.buildMachines` pointing at romeo, and the local settings that keep heavy builds off the box. |
| `nixos/romeo/remote-builder.nix` | romeo | The `nixremote` account nix logs in as: its forced-command login shell and its entry in `trusted-users`. |
| `tailscale-acl.hujson` | tailnet | `whiskey -> romeo:22` in `acls`, and the `ssh` rule that admits `nixremote` and `syncoid` — and nothing else — between servers. |

There is no SSH key, no `authorized_keys`, no extra sshd port and no agenix
secret anywhere in this. That is deliberate — see below.

## What whiskey can actually do on romeo

Three limits, outermost first. They are not equally strong and it is worth
knowing which is which.

1. **It can only log in as `nixremote`.** The `ssh` rule in the ACL is an
   explicit two-account allowlist (`nixremote`, `syncoid`). It used to read
   `["autogroup:nonroot", "root"]`, which was not the modest grant it looks
   like: besides root outright, it admitted `bcnelson`, who is in `@wheel` with
   passwordless sudo on every server. Any tagged server could have taken a root
   shell on any other.
2. **That account can only speak the build protocol.** Tailscale SSH has no
   `authorized_keys`, so there is nowhere to hang a `command="..."`; the forced
   command lives in the login shell instead. It matches exactly and re-execs a
   fixed argv, so nothing rides along appended to a command that matched.
   Arbitrary commands and interactive shells are refused.

   The accepted commands are `nix-daemon --stdio`, `nix daemon --stdio` and
   `nix-store --serve --write`. Both daemon spellings are listed because nix's
   default `remote-program` is the standalone `nix-daemon` binary, while
   `nix daemon` is what arrives when `remote-program` is set explicitly. The
   first version of this shell allowed only the second form and refused every
   build with `refused: nix-daemon --stdio` — which is exactly what the refusal
   message printing the requested command is for.
3. **But `nixremote` is a trusted nix user, and that is a large grant.** Remote
   building requires it — an unsigned derivation can only be added to the store
   by a trusted user — and a trusted user can add arbitrary paths to romeo's
   store, override daemon settings, and reach the network from a fixed-output
   derivation. So treat 1 and 2 as keeping a compromised whiskey from getting a
   shell on romeo's LAN, not as a boundary against a determined one. The
   boundary is that whiskey is trusted to build for romeo, which is the whole
   point of the arrangement.

A human SSHing from one tagged server to another is attributed to `tag:server`,
not to the person, so limit 1 also ends those hops. Reach each host directly;
`group:admin` still grants root and non-root everywhere.

## Why Tailscale SSH and not a key

Port 22 on romeo's tailnet address does not belong to romeo's sshd. tailscaled
answers it (the banner is literally `SSH-2.0-Tailscale`) and authenticates by
tailnet identity, ignoring SSH keys entirely. A key-based builder would have had
to dodge it on a second port, with a generated keypair, a committed public half
and a `just rekey` for every change — all to re-authenticate a peer WireGuard has
already authenticated.

Two consequences worth knowing:

- **A forced command has to live in the login shell.** There is no
  `authorized_keys` file to put one in, because there is no key.
- **The ACL rule must be `accept`, not `check`.** `check` mode wants a browser
  re-authentication, which in a timer-driven rebuild is just a hang. The
  `tag:server -> tag:server` rule is already `accept`; the per-user
  `autogroup:self` rule below it is `check`, and is the reason a non-interactive
  `ssh whiskey-1` can appear to hang forever.
- **Nothing pins romeo's host key.** tailscaled presents its own, which is in no
  repo and is regenerated if the node's state is rebuilt. So whiskey's
  `/etc/ssh/ssh_config` turns host-key checking off for exactly this
  destination — the same thing `tailscale ssh` itself does, for the same reason.

## What keeps builds off whiskey

`nix.settings.system-features = [ ]`. Claiming no system features means any
derivation with `requiredSystemFeatures` — `big-parallel` covers
nodejs/V8/chromium — *cannot* be built locally, so nix must hand it to romeo.
Derivations with no feature requirements still build on whiskey, which keeps the
hundreds of tiny `writeText`-shaped ones off the tailnet.

If romeo is unreachable, a `big-parallel` build fails with a clear "required
system features" error and `auto-update` retries it (three attempts, then it
reports failure to cadence). Everything else still builds. A builder outage
degrades whiskey's updates; it does not block them.

## Deploying a change to this

The ACL and the two hosts land from the same commit but not at the same time, and
the order matters:

1. Push to `main`. `tailscale.yml` applies the ACL within a minute or two.
2. Rebuild **romeo** before whiskey picks the commit up — `nixremote` has to
   exist before whiskey tries to log in as it. Romeo deploys from the
   `auto-update` branch, which CI only advances after `check-hosts` passes, so
   this can lag; `systemctl start auto-update` on romeo makes it immediate.
3. Whiskey's hourly `auto-update` does the rest.

Out of order, whiskey's rebuilds fail on `big-parallel` derivations until romeo
catches up. Nothing breaks permanently, but the hour is wasted.

## Checking it works

On whiskey:

```
nix store info --store ssh-ng://nixremote@romeo.b.nel.family
```

`Trusted: 1` is the answer you want — without it, romeo will refuse the unsigned
derivations a remote build consists of. Then watch a real build land:

```
nix build --rebuild .#nixosConfigurations.whiskey-1.config.system.build.toplevel
```

`nix-daemon` logs the offload (`building ... on 'ssh-ng://...'`), and romeo's
`journalctl -u tailscaled` shows the session.

That the forced command is doing its job:

```
ssh nixremote@romeo.b.nel.family id
```

should come back `refused: id`, not a uid.

If it fails with `refused: <something>`, the forced command in
`nixos/romeo/remote-builder.nix` does not list what this client's nix actually
sends; add that exact string. If it fails with `nix: command not found`, the
login environment Tailscale SSH builds for `nixremote` is missing the system
profile — name the binary instead of relying on `PATH`, as the builder's
`hostName`:

```
romeo.b.nel.family?remote-program=/run/current-system/sw/bin/nix%20daemon
```

(The `%20` is required — the value is one URI component, and without the
`daemon` subcommand nix invokes it with a bare `--stdio` and the connection dies
with `unrecognised flag '--stdio'`.)

## Adding another client

Whiskey is the only one today. Another host needs: `nixos/whiskey/remote-builds.nix`
copied into its own directory, and one `acls` line granting it `romeo:22`. The
`ssh` rule already admits `nixremote` from any `tag:server` host, so neither the
`ssh` section nor romeo itself needs a change.
