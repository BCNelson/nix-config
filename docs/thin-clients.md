# Thin clients

Some hosts — the Dell thin clients, ~2 GB of RAM — cannot build their own system
closure. They cannot even *evaluate* it: instantiating nixpkgs for a NixOS
configuration needs several gigabytes on its own.

So they don't. Romeo builds their closures, signs them, and publishes them to
the binary cache. A thin client only ever reads a small JSON manifest, fetches
the store path it names, and switches to it. It never clones the repo, never
runs `nix eval`, and never runs `nixos-rebuild`.

## Moving parts

| Piece | Where | What it does |
| --- | --- | --- |
| `hosts/thin-clients.nix` | repo | The list of thin-client hostnames. Single source of truth. |
| `secrets/store/romeo/nix_cache_key.{age,pub}` | repo | Cache signing key. agenix generates both halves; clients trust the `.pub` at eval time. |
| `modules/nixos/thinClientBuilder` | romeo | Builds, signs, publishes, writes manifests. |
| `modules/nixos/thinClient` | client | Polls the manifest, fetches the closure, activates it. |
| `nixos/_mixins/roles/thin-client.nix` | client | The role a thin client's `default.nix` imports. |
| `pkgs/install-system --thin` | installer | Bootstraps a new thin client. |

## How an update flows

1. A commit lands on `main`. CI checks every host, including the thin clients —
   they are ordinary entries in `nixosConfigurations`, so `check-nix-config.yml`
   already covers them with no extra wiring.
2. CI advances the `auto-update` branch to that commit.
3. Romeo's `auto-update.service` pulls and rebuilds romeo itself.
4. `thin-client-build.service` is `WantedBy=auto-update.service` and ordered
   `After=` it, so it fires the moment romeo's own rebuild finishes — not on the
   next timer tick. (A 6 h timer exists purely as a backstop.)
5. For each thin client it runs `nix build`, signs and copies the closure into
   `/var/public-nix-cache` — the document root of the `nixcache.nel.family`
   vhost — and atomically writes
   `/var/lib/thin-client-builder/manifests/<host>.json`, served at
   `https://nixcache.nel.family/thin-clients/<host>.json`.
6. `thin-client-update.timer` on the client (every 15 min) reads the manifest,
   compares `storePath` against `/run/current-system`, and if they differ:
   `nix copy --from` the cache, `nix-env -p /nix/var/nix/profiles/system --set`,
   then `switch-to-configuration switch`. Reboots itself if the kernel or initrd
   changed.

The manifest looks like this:

```json
{
  "hostname": "delta-1",
  "commit": "5053fef1e2...",
  "storePath": "/nix/store/xxx-nixos-system-delta-1-25.11",
  "status": "ready",
  "builtAt": "2026-07-28T18:04:11Z"
}
```

A failed build publishes `"status": "failed"` with the tail of the build log in
`"error"`. The client surfaces that and reports a health check failure rather
than sitting quietly on a stale closure — a host whose build has been broken for
a week should not look healthy.

## Cache signing key

The builder signs everything it publishes and the client verifies those
signatures, both at install time and on every update.

The key is an ordinary agenix secret with a generator, defined in
`nixos/romeo/services/thinClientBuilder.nix` (abridged here — the real script
uses `${pkgs.nix}/bin/nix` and passes `--extra-experimental-features`):

```nix
age.secrets.nix_cache_key = {
  rekeyFile = ../../../secrets/store/romeo/nix_cache_key.age;
  generator = {
    tags = [ "nix-cache" ];
    script = { pkgs, lib, file, ... }: ''
      priv=$(nix key generate-secret --key-name nixcache.nel.family-1)
      nix key convert-secret-to-public <<< "$priv" \
        > ${lib.escapeShellArg (lib.removeSuffix ".age" file + ".pub")}
      echo "$priv"
    '';
  };
};
```

The generator writes the *public* half to `nix_cache_key.pub` next to the
encrypted private half — the adjacent-file trick from agenix-rekey's own
wireguard example. That file is committed in the clear on purpose: clients have
to trust the key at *evaluation* time, so it cannot live inside an encrypted
secret. It is a public key, exactly like `secrets/masterKeys/*.pub`.

Normal workflow, same as any other secret here:

```
just generate-secrets     # or: agenix generate -t nix-cache
just rekey
git add secrets/store/romeo/nix_cache_key.age secrets/store/romeo/nix_cache_key.pub secrets/hosts
```

`agenix generate` encrypts to the master identities' *public* halves, so it
needs no hardware key. `just rekey` does, as always.

Romeo will not evaluate between defining the secret and rekeying it —
agenix-rekey refuses a secret whose rekeyed output is missing, with a message
saying exactly that. Deploy romeo once after rekeying, before installing any
thin client. `install-system --thin` refuses to start until the `.pub` exists.

Rotating the key means re-signing every published closure: delete both files,
regenerate, rekey, and let the builder republish (`agenix generate -f -t
nix-cache` forces regeneration). The key *name* is baked into every signature,
so leave `nixcache.nel.family-1` alone unless you mean to rotate.

## Installing a thin client

From the installer ISO:

```
install-system --thin delta-1
```

It follows the same shape as a normal install, with the pieces that need RAM
removed:

1. Picks a disk, warns about it, and runs disko — locally, on the client. Disko
   is small and this is the one heavy-ish thing the hardware handles fine.
2. Runs `nixos-generate-config` to produce the hardware configuration.
3. Writes `nixos/<prefix>/default.nix` importing the thin-client role, adds the
   host to `flake.nix` and to `hosts/thin-clients.nix`, generates an SSH host
   key and writes `hosts/data/<host>.nix`.
4. **Installs the SSH host key into `/mnt/etc/ssh` immediately**, before pushing.
   Its public half is what `hosts/data/<host>.nix` pins for agenix, and the
   private half only exists in the live session — which the resume path has to
   survive losing.
5. Does **not** rekey agenix secrets, deliberately — see "Rekeying happens on a
   workstation" below. It prints what you have to do instead, including that CI
   will fail on the branch until you do it.
6. Commits and pushes `install-<host>`. It does **not** run
   `nixos-install --flake`; there is no evaluation anywhere past this point.
7. Polls the manifest until a closure appears. Open the PR, let CI pass, merge.
8. Fetches the closure with
   `nix copy --from <cache> --to local?root=/mnt`. Writing straight into `/mnt`
   matters: the live ISO's store is a tmpfs sized to RAM, and a full system
   closure does not fit in 2 GB.
9. `nixos-install --root /mnt --system <path>` — `--system` takes a prebuilt
   closure, so nix never loads the flake.
10. Expires the users' passwords and offers to reboot.

### What the installer waits for

Not "the manifest names my commit". The branch reaches `main` through a merge or
a squash, so the installer's SHA generally never appears anywhere. What holds is
that the manifest's commit must have *moved* from whatever it advertised before
the push — and for a brand new host, that no manifest existed at all. The
installer records that baseline before pushing, which is also what stops a
*reinstall* from immediately accepting the closure built from the old config.

### Resuming

The wait spans a human merging a PR and romeo chewing through a full closure, so
interruption is expected rather than exceptional. Progress is checkpointed to
`~/.local/state/install-system/<host>.json` and to
`/mnt/var/lib/install-system/<host>.json`.

```
install-system --thin delta-1              # re-run: picks up automatically
install-system --thin --resume delta-1     # after the live ISO was rebooted
```

A plain re-run finds the session checkpoint. `--resume` covers the case where
the ISO itself was rebooted and the session copy is gone: it asks which disk was
being installed to, re-mounts it with `disko --mode mount` — which touches no
partition tables, unlike the `zap_create_mount` a fresh install uses — and reads
the checkpoint back off the disk.

## Rehearsing an install in QEMU

```
just thinInstallTest "-device qemu-xhci -device usb-host,vendorid=0x1050"
```

Builds the minimal console ISO and boots it with **2 GB of RAM and 2 cores** —
what the Dell thin clients actually have — against a persistent 32 GB disk at
`test/working/iso_console/iso.qcow2`. The `extraArgs` above pass a YubiKey
through, which `install-system` needs to unlock the repo. Drop that argument if
you unlock with GPG instead.

Then, at the console: `install-system --thin <host>`.

Two things have to be true before the rehearsal can get past the polling step,
because the installer clones `main` from GitHub rather than using your working
tree:

1. The thin-client work is merged. The ISO carries an `install-system` built
   from your local flake, so `--thin` exists — but the *clone* needs
   `nixos/_mixins/roles/thin-client.nix` and `hosts/thin-clients.nix`, or the
   new host will not evaluate when agenix-rekey runs.
2. `secrets/store/romeo/nix_cache_key.pub` is committed and pushed. Without it
   the installer refuses to start, which is the intended behaviour.

Romeo does not need the builder enabled beforehand. Adding the first host to
`hosts/thin-clients.nix` is what turns it on, and the installer's own push is
what does that: merge the PR → CI → `auto-update` advances → romeo rebuilds
(builder now enabled) → `thin-client-build` publishes → the VM, still sitting in
its polling loop, picks the closure up and installs it.

The disk persists across runs, so an interrupted rehearsal resumes with
`install-system --thin --resume <host>`. Delete the qcow2 to start clean.

`just isoTest <version> <memory> <cores> <diskSize> <extraArgs>` is the general
form if you want different limits; `thinInstallTest` is just that with the thin
client's numbers filled in.

### The swap prompt does nothing on the unencrypted path

`install-system` asks for a swap size and passes it to disko as
`--arg swapSize`, but `disko/default.nix` takes `{ disk, ... }` and never reads
it: the argument is swallowed by the ellipsis and root takes 100% of what is
left after the 1 GB ESP. Only `disko/luks.nix` declares `swapSize` and creates a
swap partition.

So an unencrypted thin client has **no on-disk swap at all**, during the install
or afterwards. zram is not a supplement to it, it is the whole of it. That is
survivable -- and on an eMMC with limited write cycles, arguably preferable --
and it no longer has to carry an agenix-rekey run, since that moved off the
thin client entirely.

### Rekeying happens on a workstation, never on the thin client

`install-system --thin` does not rekey. It used to offer FIDO2 or `--dummy`;
both are gone, because both were wrong.

agenix-rekey evaluates every host in the flake, which forces a local build of
`nixpkgs-patched` — `applyPatches` over `patches/`, which is in no binary cache.
On the installer ISO the nix store *is* RAM: `/nix/store` is an overlay whose
upperdir is a tmpfs. So rekeying means pushing a whole nixpkgs tree through the
memory of a 2 GB machine. Measured in a VM: it livelocks, `kswapd0` and nix's GC
marker both in hung-task reports, 26 minutes of no progress and no OOM kill to
end it. Raising the tmpfs size does not help — the store growing *is* the memory
being consumed.

`--dummy` avoided the livelock but produced placeholder secrets you had to redo
from a workstation anyway, so it bought nothing except a false sense of progress.

So there is a human step in the middle:

```
# on the thin client -- pushes install-wyse-1, then sits in its polling loop
install-system --thin wyse-1

# on a workstation, with the security key
git fetch && git checkout install-wyse-1
just rekey
git add secrets/hosts && git commit -m 'rekey wyse-1' && git push

# now CI can pass -> merge -> auto-update advances -> romeo builds
# -> the thin client's poll finds the manifest and finishes on its own
```

**CI fails on the branch until you rekey**, because it builds every host and this
one has no secrets yet. That is expected, not a broken build.

### Note on the ISO

`nixos/iso_console` enables `zramSwap` at 100% of RAM and mounts the writable
store layer with `size=4G,nr_inodes=0`. All three matter:

- Without zram there is no swap at all until disko runs, and on the unencrypted
  path not even then.
- The default store tmpfs is half of RAM — ~983 MB — which is not enough for the
  disko step's toolchain fetch.
- `nr_inodes` defaults to half the RAM *pages*: 251523 on a 2 GB box. Unpacking
  nixpkgs source trees is tens of thousands of tiny files, so without
  `nr_inodes=0` you get "No space left on device" at 24% of bytes used with
  700 MB of RAM still free. That one cost a debugging cycle.

## Operating notes

- **`max-jobs = 0` on clients.** A thin client should never compile anything; a
  local build means the closure was not published properly. Failing loudly beats
  thrashing into the OOM killer.
- **GC roots.** The builder keeps `/nix/var/nix/gcroots/thin-clients/<host>` so
  romeo's own garbage collection cannot delete NARs a live manifest still
  advertises. The client keeps `/nix/var/nix/gcroots/thin-client-current` so a
  GC between switching and rebooting cannot remove the closure it is about to
  boot into. Removing a host from `hosts/thin-clients.nix` drops both its root
  and its manifest on the next build.
- **Retry state.** `/var/lib/thin-client-update/state` counts attempts per
  published store path, so a closure that fails to activate is retried three
  times rather than once, and a newly published closure resets the counter —
  same reasoning as `auto-update.sh`.
- **Disk headroom.** An update holds the old and new closures at once. On a
  small eMMC that is the constraint to watch, not RAM. The role turns off
  `keep-outputs`/`keep-derivations`; the daily `nix.gc` from `nixos/common.nix`
  (`--delete-older-than 7d`) does the rest. Shorten that per host if it gets
  tight.
- **home-manager packages come from the system closure.** Thin clients set
  `home-manager.useUserPackages = true` (via the `thinClient` flag in
  `lib/default.nix`). They must: `max-jobs = 0` means nothing can be built
  locally, and home-manager's default activation realises a `user-environment`
  symlink tree that can never be substituted from a cache, so
  `home-manager-<user>.service` failed on every boot with
  `Cannot build '...-user-environment.drv': local builds are disabled`.
  With `useUserPackages`, `home.path` becomes an input to the system closure
  that romeo builds, and activation has nothing left to realise.

  Activation still calls `nixProfileRemove home-manager-path`, which is a no-op
  only when the user has no existing nix-env profile. Every thin client has this
  setting from its first boot, so that holds -- but if you ever toggle the flag
  on a host that was working without it, clear the profile
  (`nix-env -e home-manager-path`) first, or the removal will realise a
  `user-environment.drv` and hit `max-jobs = 0` once.
- **Manual poke.** `systemctl start thin-client-update` on the client;
  `systemctl start thin-client-build` on romeo.
- **Where things live.** Manifests:
  `/var/lib/thin-client-builder/manifests/`. Published NARs:
  `/var/public-nix-cache/`. Both are served by the `nixcache.nel.family` vhost,
  which is LAN-only (`nixos/romeo/unbound.nix` points it at 192.168.3.7).
