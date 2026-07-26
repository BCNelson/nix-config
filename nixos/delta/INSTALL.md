# Installing delta-* (Dell Wyse 3040), a limited host

A **limited host** is one that cannot *evaluate* this flake — not merely one
that cannot build it. Evaluating this repo with home-manager peaks well above
the 2 GiB a Wyse 3040 has, before a single derivation is built.

One invariant follows, and the whole design is downstream of it: every
operation on such a host is *fetch a store path someone else computed, realise
it, activate it*. No `nix build`, no `nixos-rebuild`, no `nixos-install
--flake`. That applies to the first install exactly as it does to every update
afterwards — the same contract used twice:

    GET https://nixcache.nel.family/system/delta-1  ->  a store path,
    whose closure is in that same cache.

romeo produces it (`services.bcnelson.closurePublisher`). `install-system
--limited` consumes it once; `services.bcnelson.remoteUpdate` consumes it
forever after.

## What that costs you up front

The closure is built *before* the machine exists, so anything normally minted
at install time has to be answered in advance:

| Normally minted at install | Answer here |
|---|---|
| Filesystem UUIDs, from `mkfs` | Partition **labels**, which we choose — `disks.nix` writes them, `filesystems.nix` names them |
| The hardware scan | Generated on the target and pushed *before* the build; the install is inherently two-phase |
| The SSH host key | `install-system` generates it, commits it, and plants it on the target afterwards |

## Installing

One command, which pauses in the middle waiting for you to merge:

```sh
install-system delta-1 --limited
```

Boot `iso_console` on the machine first (`just isoCreate iso_console`); BIOS is
F2, default password `Fireport`, enable USB boot and leave it on UEFI. The 3040
ships 64-bit firmware, so the standard hybrid image boots — if it refuses
outright, suspect 32-bit UEFI, which this image cannot boot at all.

What it does, in order:

**On the machine.** Clones the repo, unlocks it (your FIDO2 or GPG key has to
be physically present), partitions with disko, generates
`1.hardware-configuration.nix` with `--no-filesystems`, generates the SSH host
key into `hosts/data/delta-1.nix`, inserts the host into `flake.nix`, commits
to `install-delta-1`, pushes, opens a pull request — then polls for the
closure. It skips `nix fmt` and the dummy rekey, which both evaluate
everything.

`--no-filesystems` matters: the generated `fileSystems` would name UUIDs and
collide with the labels in `filesystems.nix`. On a normal install that surfaces
immediately in `nixos-install` and gets hand-edited out (see
`nixos/whiskey/1.hardware-configuration.nix`); here it would only surface on
romeo, long after the installer started waiting, on a machine already wiped.

**Then you, on a workstation.** Check out the branch, `just rekey` with the
yubikey, push, merge. Without this romeo's build fails on unrekeyed secrets —
delta needs its ntfy topics from first boot.

**Then romeo, unattended.** The ntfy refresh starts `auto-update`, which pulls
main; `closure-publisher` runs after it, works out which hosts consume
published closures (every one with `remoteUpdate` enabled — no list to
maintain), builds, signs, copies into the cache and publishes the manifest.

**Then the machine resumes.** Copies the closure straight into `/mnt` with
`TMPDIR` on the target disk, checks every device in the closure's `fstab`
actually exists, sets the profile, writes `/etc/NIXOS` and `/etc/mtab`,
installs the bootloader through `nixos-enter`, plants the SSH host key, and
expires the user passwords. No `/config` is copied: a limited host updates from
closures, not a checkout.

`--wait-minutes` defaults to 90 and covers your merge as well as the build.

## Afterwards

Pin the cache signing key, which is generated on romeo's first publish:

```sh
just cache-key
```

Put it in `trustedPublicKeys` in `nixos/delta/default.nix` and set
`checkSignatures = true`. Until then the closure is trusted on the strength of
the HTTPS connection to romeo alone — note that only relaxes that one fetch,
the host's global `require-sigs` stays on.

## Steady state

Nothing polls tightly. A push drives the chain: ntfy starts `auto-update` on
romeo → `closure-publisher` runs after it and republishes → the same ntfy
message reached delta, which scheduled a single check for 30 minutes later,
long enough for both. If the store path changed it downloads and activates,
rebooting only if the kernel or initrd did.

The 6h timers on both sides are backstops for pushes that were missed because a
host was off or a publish ran long. If romeo routinely needs more than 30
minutes from push to published, raise `ntfy-refresh.delay` rather than
shortening the poll.

```sh
journalctl -u remote-update -f                 # on the client
journalctl -u remote-update-ntfy-client -f     # on the client
journalctl -u closure-publisher -f             # on romeo
systemctl list-timers remote-update-delayed    # a pending post-push check
just published-closures                        # what each host is told to run
```
