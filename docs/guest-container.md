# Guest container: bwnelson

A sandboxed NixOS box on romeo for bwnelson (brother) — coding, learning
Linux, and running [cyclus](https://fuelcycle.org/) fuel-cycle simulations.
Reached over Tailscale — on **his own tailnet**, not ours; no ports are exposed
to the internet.

Config lives in [`nixos/romeo/containers/bwnelson.nix`](../nixos/romeo/containers/bwnelson.nix).

## Design in one paragraph

A declarative `systemd-nspawn` container, not a microvm: he is trusted against
malice but not against mistakes, and the shared kernel only matters against
someone actively attacking it. `privateUsers = "pick"` maps container root to
an unprivileged UID on romeo, so he gets real root in his sandbox without
having root on the host. The whole container filesystem is one ZFS dataset
under `vault/data/level4`, which gets sanoid snapshots (72 hourly / 31 daily /
24 weekly / 12 monthly) but no borg job — rollback without his data landing in
the offsite repos. Hard cgroup limits on the container's systemd unit are what
keep a runaway simulation from touching the family's photos and media.

## One-time bring-up

### 1. Create the dataset

Datasets in this repo are created imperatively; only the sanoid policy is
declarative. On romeo:

```bash
sudo zfs create \
  -o mountpoint=/var/lib/nixos-containers/bwnelson \
  -o quota=250G \
  vault/data/level4/bwnelson
```

`quota` is the point of the exercise — cyclus writes large sqlite/HDF5 output
per run, and without this the container root would sit on romeo's **ext4 root
partition** and could fill it. Compression is inherited from the parent.

No sanoid change is needed: the `common` template in `nixos/romeo/backups.nix`
sets `recursive = true`, so a child of `vault/data/level4` is snapshotted
automatically.

### 2. Deploy

```bash
just check-host romeo-2   # or: nix build .#nixosConfigurations.romeo-2....
```

Push and let romeo's hourly auto-update pick it up, or `just update-os` on the
host. Nothing goes in `tailscale-acl.hujson` — the node is on his tailnet.

### 3. Join it to his tailnet

Interactive, once. State persists in the container's `/var/lib/tailscale`, so
it survives romeo's reboots.

```bash
sudo nixos-container root-login bwnelson
tailscale up --ssh
```

Send him the login URL it prints; he signs in with his own account and the
node lands on his tailnet as `bwnelson`. His tailnet's default policy lets its
owner SSH to his own nodes, so from there:

```bash
ssh bwnelson@bwnelson
```

Tailscale SSH defaults to `check` mode there, so he may be sent to a browser
to re-auth occasionally — that is his policy to change. Add a real key to
`users.users.bwnelson.openssh.authorizedKeys.keys` whenever he sends one.

### 4. Install cyclus

cyclus is not in nixpkgs. `micromamba` is preinstalled and `programs.nix-ld` is
on, which is what lets conda-forge's prebuilt binaries find a dynamic loader.
As him:

```bash
micromamba shell init -s bash && exec bash
micromamba create -n cyclus -c conda-forge cyclus cycamore
micromamba activate cyclus
cyclus --version
```

This is the install path upstream documents, so his tutorials will match. If a
conda package fails to link, add the missing library to
`programs.nix-ld.libraries` in the container config.

If the modeling turns into something long-lived, packaging cyclus properly in
`pkgs/` is the better home — every dependency is already in nixpkgs (`boost`,
`libxmlxx3`, `hdf5`, `sqlite`, `cbc`, `clp`, `python3`, `cython`) and it builds
with CMake.

## Operating it

```bash
# get in as admin
sudo nixos-container root-login bwnelson

# is it up?
systemctl status container@bwnelson

# what is it actually using against its limits?
systemd-cgtop /system.slice/container@bwnelson.service
systemctl show container@bwnelson -p MemoryMax -p MemoryHigh -p MemoryCurrent

# he deleted his homework
zfs list -t snapshot vault/data/level4/bwnelson
```

## Things worth knowing

**romeo reboots when an update needs it.** `services.bcnelson.autoUpdate`
checks hourly (`refreshInterval = "1h"`) and, with `reboot = true`, reboots
only when the new generation changes the kernel, initrd or kernel modules — in
practice every few days with nixpkgs bumps. `restartIfChanged = false` stops
other updates from bouncing the container, but a reboot still restarts it.
Long cyclus runs can be cut — hence `tmux` and `mosh` in his default packages.
Tell him. If his simulations routinely run for days, the runs want a systemd
service that resumes.

**He can't reach the rest of your LAN.** The container is on his tailnet and
he's root in it, so without a block he could advertise `192.168.3.0/24` as a
subnet route (or run an exit node) and hand the whole LAN to his tailnet — and
nothing in ours would show it. romeo's firewall drops `ve-bwnelson` →
`192.168.3.0/24` in `FORWARD`. Traffic to romeo's own `192.168.3.7` goes through
`INPUT`, so unbound and romeo's opened ports stay reachable.

**Builds inside the container run on romeo's nix-daemon.** The module
bind-mounts `/nix/store` read-only but also passes the daemon socket, so
`nix build` works — it just executes on the host and its CPU use is not covered
by the container's `CPUWeight`. Same trust boundary as any unprivileged local
user, so it's not a security problem, but it does mean a heavy `nix build` of
his is less contained than the rest of his workload.

**`privateUsers = "pick"` shifts UIDs on disk.** First boot may chown the
container root into the picked range. Trivial on a fresh dataset; expect a pause
if it ever runs against a populated one.

**Resource limits are tuned for a 96 GB romeo** — `MemoryMax = 24G` with
`MemoryHigh = 20G` beneath it so he is throttled and reclaimed before anything
is killed. romeo has **no swap at all** (`swapDevices = [ ]`, no zram), so
without `MemoryMax` the OOM killer would pick its victim by badness score
across the whole box, where immich's postgres and jellyfin are the fattest
targets. `CPUWeight = 50` rather than a hard quota lets a long simulation use
the whole machine when it's quiet but yield instantly to jellyfin transcoding
or frigate detection. Dial `MemoryMax` down if ZFS ARC pressure shows up in
monitoring.
