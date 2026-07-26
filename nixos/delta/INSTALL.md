# Installing a delta-* thin client (Dell Wyse 3040)

These machines have 2 GiB of RAM and an 8 GiB eMMC. They cannot build this
flake and they cannot evaluate it either, so nothing here runs `nixos-rebuild`
or `nixos-install --flake` on the device — not even to install it. romeo builds
everything; the thin client only downloads.

## Order of operations

Installing a normal host is: install, then `nixos-generate-config`, then build.
delta-1 cannot evaluate this flake, so its closure has to be built elsewhere
*before* it can be installed — which puts the hardware scan and the host key,
both of which normally come from the installed machine, ahead of the first
build. Two things are therefore provisional until the machine exists:

| | provisional value | replaced in |
|---|---|---|
| `nixos/delta/1.hardware-configuration.nix` | written from the spec sheet | step 2 |
| `hostKey` in `hosts/data/delta-1.nix` | placeholder ed25519 key | step 5 |

The machine stays on the installer ISO from step 1 through step 4.

## 1. Boot the installer

```sh
just isoCreate iso_console     # or `just isoInstall iso_console` for a ventoy stick
```

BIOS is F2 at power-on, default password `Fireport`. Enable USB boot, disable
Secure Boot, and leave boot mode on UEFI — the 3040 ships 64-bit firmware, so
the stock hybrid image boots. (Many other Cherry Trail devices have 32-bit
UEFI, which this image cannot boot at all; if it refuses, that is the thing to
check first.)

2 GiB is enough but not generous: the ISO runs its store from a tmpfs, so
only the disko script gets copied into RAM. The system closure goes straight
to disk, which is what keeps this within budget.

The installer needs `curl`, `nix`, `nix-env`, `nixos-enter` and `mountpoint`;
the script checks for all five up front rather than failing after the disk has
already been erased.

`iso_console` brings up Tailscale SSH on boot and ntfys the auth URL, so
approve that and you can drive the rest from a workstation.

## 2. Capture the hardware configuration

The checked-in `1.hardware-configuration.nix` is written from the spec sheet,
not scanned from the machine. Replace it now, while the ISO is running and
before anything gets built:

```sh
just thin-hwconfig delta-1 root@<iso-address>
git diff nixos/delta/1.hardware-configuration.nix
```

Review the diff — `--no-filesystems` is passed because partitioning is
declared by `disks.nix`, so the result should contain kernel modules and
platform defaults but no `fileSystems` entries. Commit it.

Skipping this is how you get a system that installs cleanly and then cannot
find its root filesystem, which on a host with no local build capability means
another full publish cycle to fix.

## 3. Get the host published

`hosts/data/delta-1.nix` still holds a **placeholder** host key. That is fine
for building the first closure — the secrets are simply encrypted to a key the
device does not have, and step 5 fixes it — but the rekeyed files have to exist
or the build fails:

```sh
just rekey                     # needs the yubikey
just check-host delta-1        # should evaluate cleanly now
```

Commit and push, then let romeo publish:

```sh
just publish-closures          # or wait for autoUpdate to pull and chain into it
just published-closures
```

That should print a `/nix/store/…-nixos-system-delta-1-…` path.

## 4. Install

romeo publishes an installer script next to the closure, with the cache URL and
hostname baked in. On the booted installer, as root:

```sh
curl -fsSL https://nixcache.nel.family/system/delta-1.install | bash
```

It asks you to type the hostname first, because the disko step erases
`/dev/mmcblk0`. Then it runs the published disko script and does what
`nixos-install` does, minus the parts that build:

1. Copies the closure from the cache **straight into `/mnt`**, never into the
   ISO's own store, which is a tmpfs and would exhaust the 2 GiB.
2. Points `TMPDIR` at the target disk, for the same reason — nix unpacking
   NARs into the ISO's RAM tmpfs is the other way this runs out of memory.
3. Checks that every device in the closure's `fstab` actually exists, so a
   partitioning mismatch surfaces here rather than as an emergency shell
   after the first reboot.
4. Sets the system profile, creates `/etc/NIXOS` (without it
   `switch-to-configuration` refuses to run) and `/etc/mtab`.
5. Installs the bootloader through `nixos-enter`, with the same
   `mount --rbind` / `--make-rslave` handling `nixos-install` uses to keep
   absolute paths resolvable inside the chroot.
6. Offers to set a root password.

The script is plain text if you would rather read it before running it:

```sh
curl -fsSL https://nixcache.nel.family/system/delta-1.install | less
```

Log in as `bcnelson` with the initial password from
`nixos/_mixins/users/bcnelson`.

## 5. Register the host key

```sh
ssh-keyscan -t ed25519 delta-1
```

Paste the key into `hosts/data/delta-1.nix`, then `just rekey` and push. romeo
republishes, the client picks it up, and from then on it can actually decrypt
its secrets.

## 6. Pin the cache signing key

Until this is done, `services.bcnelson.remoteUpdate.checkSignatures` is `false`
and the closure is trusted purely on the HTTPS connection to romeo.

```sh
just cache-key
```

Put the output in `trustedPublicKeys` in `nixos/delta/default.nix` and set
`checkSignatures = true`.

## Steady state

Nothing polls tightly. A push drives the whole chain:

1. The ntfy refresh message starts `auto-update` on romeo, which pulls
   `/config` and rebuilds romeo.
2. `closure-publisher` is `WantedBy=`/`After=` `auto-update.service`, so it
   runs next, on the checkout that was just pulled, and republishes the
   manifests.
3. The same message reached delta, which scheduled a one-shot check for 30
   minutes later — long enough for steps 1 and 2 to finish. It fetches the
   manifest, and if the store path changed, downloads the closure and
   activates it, rebooting if the kernel or initrd changed.

The timers behind that are backstops only: 6h on both the publisher and the
client, for pushes that were missed because a host was off or a publish ran
long. If romeo routinely takes more than 30 minutes to get from push to
published, raise `ntfy-refresh.delay` on delta rather than shortening the poll.

```sh
journalctl -u remote-update -f                 # on the client
journalctl -u remote-update-ntfy-client -f     # on the client
journalctl -u closure-publisher -f             # on romeo
systemctl list-timers remote-update-delayed    # a pending post-push check
```
