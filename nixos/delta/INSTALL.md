# Installing a delta-* thin client (Dell Wyse 3040)

These machines have 2 GiB of RAM and an 8 GiB eMMC. They cannot build this
flake and they cannot evaluate it either, so nothing in this procedure runs
`nixos-rebuild` or `nixos-install --flake` on the device. Everything is built
on romeo and downloaded as a finished closure.

## 0. Prerequisites

`hosts/data/delta-1.nix` ships a **placeholder** host key, because the real one
does not exist until the machine has booted once. That is fine for getting the
first closure built — the secrets are simply encrypted to a key the device does
not have, and step 4 fixes it — but the rekeyed files have to exist or the
build fails:

```sh
just rekey                     # needs the yubikey
just check-host delta-1        # should evaluate cleanly now
```

Then let romeo publish the host at least once:

```sh
just publish-closures          # or wait for the 30m timer
curl -fsSL https://nixcache.nel.family/system/delta-1
```

That last command should print a `/nix/store/…-nixos-system-delta-1-…` path.

## 1. Boot the installer

Boot `iso_console` on the thin client (`just isoCreate iso_console`) and get it
on the network. The Wyse 3040 is 64-bit UEFI; disable Secure Boot in the BIOS
(F2 at power-on, default password `Fireport`).

## 2. Partition

The disko script is published next to the closure, so it can be fetched and run
directly. **This destroys everything on `/dev/mmcblk0`.**

```sh
cache=https://nixcache.nel.family
disko=$(curl -fsSL "$cache/system/delta-1.diskoScript")
nix copy --no-check-sigs --from "$cache" "$disko"
"$disko"
```

`/mnt` and `/mnt/boot` are mounted when this finishes.

## 3. Install the closure

Copy straight from the cache into `/mnt` — never into the installer's own
store, which is a tmpfs and would exhaust the 2 GiB.

```sh
system=$(curl -fsSL "$cache/system/delta-1")
nix copy --no-check-sigs --from "$cache" --to /mnt "$system"
nix-env --store /mnt -p /mnt/nix/var/nix/profiles/system --set "$system"
nixos-enter --root /mnt -- /run/current-system/bin/switch-to-configuration boot
```

Set a root password (`nixos-enter --root /mnt -- passwd`) if you want console
access, then reboot.

## 4. Register the host key

The `hostKey` in `hosts/data/delta-1.nix` is a placeholder until the machine
exists. On a workstation:

```sh
ssh-keyscan -t ed25519 delta-1
```

Paste the key into `hosts/data/delta-1.nix`, then `just rekey` and push. romeo
picks up the change on its next publish, the client pulls the new closure on
its next tick, and from then on it can actually decrypt its secrets.

## 5. Pin the cache signing key

Until this is done, `services.bcnelson.remoteUpdate.checkSignatures` is `false`
and the closure is trusted purely on the HTTPS connection to romeo.

```sh
just cache-key
```

Put the output in `trustedPublicKeys` in `nixos/delta/default.nix` and set
`checkSignatures = true`.

## Steady state

romeo republishes on the same ntfy refresh topic autoUpdate listens on, so a
push reaches the thin clients on their next poll rather than waiting out the
30m publish timer. `closure-publisher-ntfy-client` waits for
`auto-update.service` to finish pulling `/config` before it starts building —
both subscribers wake on the same message, and without that wait the publisher
can win the race and republish the commit that was already published.

`remote-update.timer` fires every 15 minutes on the client: it fetches
`https://nixcache.nel.family/system/delta-1`, compares it with
`/run/current-system`, and if they differ downloads the closure and activates
it, rebooting when the kernel or initrd changed. Watch it with

```sh
journalctl -u remote-update -f                    # on the client
journalctl -u closure-publisher -f                # on romeo
journalctl -u closure-publisher-ntfy-client -f    # on romeo
```
