# `systemd.sysusers` for system-manager

An upstream candidate for [numtide/system-manager](https://github.com/numtide/system-manager),
kept here verbatim so it can be lifted into a PR without edits. Until it lands
upstream, `system-manager/default.nix` imports `module.nix` directly.

| file                 | upstream path                             |
| -------------------- | ----------------------------------------- |
| `module.nix`         | `nix/modules/sysusers.nix`                |
| `container-test.nix` | `testFlake/container-tests/sysusers.nix`  |

`module.nix` also needs adding to the `imports` list in `nix/modules/default.nix`,
and `container-test.nix` registering in `testFlake/container-tests/default.nix`.
Option documentation is generated from the module, so no docs file is needed.

## The gap it fills

system-manager writes `/etc/passwd` and `/etc/group` only through userborn.
userborn is full declarative account management: it takes ownership of the
users and groups it declares, updates their GECOS, shell and home, and removes
the members it added on deactivation. It is careful about what it did not
create — upstream's `existing-group-members` container test pins that
pre-existing members survive activation and deactivation — but it is still a
much bigger commitment than some hosts want.

The common small case has no good answer today: a foreign-distro host that just
needs *one more entity to exist* — a group named by a udev rule, or a system
user for a service — while leaving `/etc/passwd` and `/etc/group` to the
distribution. Enabling userborn for that also pulls in the whole
`users-groups.nix` tree, which on a non-NixOS host means ~25 stock groups
declared at NixOS GIDs, plus rewriting the admin user's GECOS and pinning their
login shell to a `/nix/store` path that churns on every bump.

`systemd-sysusers` is the distro-native fit, and its limitation is the feature:
it is strictly additive. Per `sysusers.d(5)`, it "will do nothing if the
specified users or groups already exist". It cannot renumber a GID, cannot drop
a member, and has no removal path at all — so it is safe to point at an
`/etc/group` that something else owns.

## Shape

Mirrors `nix/modules/tmpfiles.nix`, which already exposes the same
rules/settings/packages triple for `systemd-tmpfiles`:

```nix
systemd.sysusers.rules = [ "g plugdev -" ];

systemd.sysusers.settings."10-mypackage" = {
  plugdev.g = { };
  myservice.u = {
    id = "404";
    gecos = "My service";
    home = "/var/lib/my-service";
  };
  alice.m.id = "wheel";
};

systemd.sysusers.packages = [ pkgs.nginx ];
```

The module defines a `system-manager-sysusers.service` oneshot wanted by
`system-manager.target` rather than leaning on systemd's own
`systemd-sysusers.service`. The built-in unit is gated on
`ConditionNeedsUpdate=|/etc`, which fires when `/usr` is newer than `/etc` —
after a package update — and writing into `/etc/sysusers.d` does not set that
flag. Observed on Fedora 43, it logs `skipped, no trigger condition checks were
met` on a normal boot, so entries would land only whenever an unrelated
distribution update happened to touch `/usr`. An explicit unit also means
activation does not have to wait for a reboot.
It is only defined when there is something to create, so enabling system-manager
does not start running sysusers on hosts that never asked for it. The generated
directory is pinned into the unit via `Environment=SYSUSERS_D=`, so the unit
file changes whenever the entries do and system-manager's restart-if-changed
logic re-runs it.

## SELinux

On an SELinux-enforcing host the activation unit needs a domain allowed to
write `shadow_t`. Without one it runs as `init_t`, gets through `/etc/group`,
and then fails:

```
systemd-sysusers[…]: Creating group 'plugdev' with GID 969.
systemd-sysusers[…]: Failed to open /etc/gshadow: Permission denied
```

The module deliberately does not hardcode a context — that is policy-specific —
so this is left to the consumer. In this repo `system-manager/_mixins/selinux.nix`
sets `SELinuxContext=system_u:system_r:useradd_t:s0` and
`pkgs/nix-store-selinux.nix` makes the `/nix/store` binary a legal transition
target. Upstream may want to document this in the option description rather
than solve it.

### Why `systemd.sysusers.executable` exists

`useradd_t` is the natural domain, but it is only safe with a
libselinux-linked binary — which is what that option is for.

`pkgs.systemd` is built with `withSelinux = false`, so its `systemd-sysusers`
never calls `setfscreatecon()`, and it writes a temp file and renames it into
place. The label therefore comes from a type transition, and `useradd_t` carries
an unnamed catch-all whose named exceptions only ever match the *final*
filenames:

```
type_transition useradd_t etc_t:file passwd_file_t group;   # named — never fires
type_transition useradd_t etc_t:file shadow_t;              # default — fires
```

So `/etc/group` is created `shadow_t` and `rename` preserves it.
`system_dbusd_t` cannot read `shadow_t`, so dbus-broker fails to start, which
takes down the display manager — and logind needs the bus, so there is no TTY
either. This bricked a Fedora 43 host on 2026-07-31; recovery required a
ZFSBootMenu chroot and a manual `chcon`. Rolling back generations does not help,
because SELinux labels are not tracked by nix.

Pointing `executable` at `pkgs.systemd.override { withSelinux = true; }` makes
the binary set its own create context, so the transition never applies and the
label is correct regardless of domain. That is a real fix rather than a
mitigation; relabelling afterwards is not a substitute, since `useradd_t` does
not hold `relabelto` on `passwd_file_t` and would be denied on the one file
that matters.

Check with `systemd-sysusers --version`, which reports `-SELINUX` for the stock
build and `+SELINUX` with the override. Do not check with `ldd`: systemd
dlopens libselinux, so nothing appears in `DT_NEEDED` even when support is
present.

The host's own `/usr/bin/systemd-sysusers` is also built with libselinux and
looks like a free alternative, but it is not usable: `SELinuxContext=` performs
a domain transition, which requires `entrypoint` on the target binary's type.
`useradd_t` holds `entrypoint` on `nix_store_t`, `user_home_t` and
`useradd_exec_t` — not `bin_t` — so it is denied:

```
avc: denied { entrypoint } for comm="(sysusers)" path="/usr/bin/systemd-sysusers"
  scontext=system_u:system_r:useradd_t:s0 tcontext=system_u:object_r:bin_t:s0
```

Granting `entrypoint` on all of `bin_t` would make every system binary a legal
way into the user-management domain, so the `/nix/store` build is the right
trade even though it costs a compile.

The same hazard applies to `services.userborn.enable`, which is also not built
with libselinux and has no host-provided equivalent.

## Verification done here

- Renders and `systemd-sysusers --dry-run` accepts every entry type — `g`, `u`,
  `m`, and raw `rules` lines — with GECOS quoting and trailing unset columns
  trimmed, so `m alice wheel` does not render as `m alice wheel - - -`.
- Confirmed the create-only claims directly on a Fedora 43 host: a `g` line
  naming an existing group is silently skipped, and a line requesting a GID
  that is already taken falls back to an allocated GID rather than failing.
  `systemd-sysusers` exits 0 in both cases.
- `systemd-sysusers.service` is ordered `Before=systemd-udevd.service`, so
  groups referenced by udev rules exist before udevd parses them at boot. This
  repo relies on that for `hardware.keyboard.qmk.enable`, whose upstream QMK
  rules reference `plugdev`; udev drops a whole rule line whose `GROUP=` it
  cannot resolve.
- The container test covers creation, non-modification of a pre-existing group
  with a different GID and an unknown member, idempotent re-activation, and
  deactivation. It has not been executed here — the container tests need
  `systemd-nspawn` and root.

## Contributing

Per `CONTRIBUTING.md`, open an issue first, then branch as `<USER>/<issue>`.
