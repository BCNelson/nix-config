---
name: bump-package
description: Carry a package at a newer version than nixpkgs ships, as an overlays/bumps/<attr>.nix file that hands back to nixpkgs automatically once the channel catches up. Use when asked to update, bump, pin or "get the latest" of a nixpkgs package, to vendor an unmerged nixpkgs PR, or when an "overlays/bumps/... is obsolete" warning shows up.
---

# Bump a package ahead of nixpkgs

Every version bump is one file, `overlays/bumps/<attr>.nix`, named after the
top-level package attribute. `overlays/bumps/default.nix` picks it up and
applies the same rule to every bump. Never write your own version comparison
in `overlays/default.nix`:

- **Ours is used** only while it is *strictly newer* than nixpkgs' version and
  `meta.platforms` / `badPlatforms` allow the host. Otherwise the attribute is
  nixpkgs' own, untouched, and comes from the binary cache.
- **A tie goes to nixpkgs**, unless our build sets `passthru.preferOnTie = true`.
- **Once nixpkgs catches up**, evaluating the attribute warns
  `overlays/bumps/<attr>.nix is obsolete ... Delete it.`
- **Only `version` and `meta` are read to decide**, so a bump that has been
  overtaken is never fetched or built.

## 1. Is a bump the right tool?

- **Check what nixpkgs has first.** Use the `nix` MCP tool
  (`{"action":"info","query":"<attr>","channel":"unstable"}`) and
  `nix eval --raw .#nixosConfigurations.<host>.pkgs.<attr>.version`. If the
  channel already has the version, there is nothing to do.
- **Bump vs. modification.** A bump is for a *newer version*. A fix to
  nixpkgs' own version (a patch, a dropped test, a relaxed dependency, a
  wrapper) belongs in `modifications` in `overlays/default.nix`. That
  overlay runs after the bumps, so a fix there applies to whichever version won.
- **Top-level attributes only.** Nested sets like `python3Packages.foo` or
  `linuxPackages_zen` aren't handled here. Override those in `modifications`
  as before.
- **Packages nixpkgs doesn't have at all** go in `pkgs/` plus
  `pkgs/default.nix`, not here.

## 2. Pick the shape

The file is `{ final, prev, inputs, lib }: <derivation>`. Take only the
arguments you use, because deadnix flags unused ones. Above the code, write a
comment saying **why we can't wait** for nixpkgs (the bug or feature, with an
upstream link or PR number) and **where the version came from**.

**A. Version + src override.** Use this for prebuilt binaries, simple source
builds, and plain-bump PRs. Take the version from `prev.<attr>`:

```nix
# Upstream 1.4.0 fixes <bug> (<link>); nixpkgs PR #NNNNN is a plain bump.
{ final, prev, ... }:
prev.foo.overrideAttrs (finalAttrs: {
  version = "1.4.0";
  src = final.fetchFromGitHub {
    owner = "foo";
    repo = "foo";
    tag = "v${finalAttrs.version}";
    hash = "sha256-...";
  };
})
```

A builder with a second fixed-output hash (Go `vendorHash`, Rust `cargoHash`,
npm `npmDepsHash`) also needs that hash changed. Check that it actually
changes: the build should fail with a hash mismatch for the deps derivation
after you change `src`. If it builds the old deps instead, replace the deps
derivation itself. For example, for Rust:
`cargoDeps = final.rustPlatform.fetchCargoVendor { inherit (finalAttrs) src; hash = "..."; };`
For npm: `npmDeps = final.fetchNpmDeps { ... }`.

**B. Vendored nixpkgs PR.** Use this when the PR changes more than version and
hashes (new inputs, postPatch, test skips). Copy the PR's `package.nix` to
`pkgs/<attr>/package.nix`, unchanged where possible, and mark any local edits
in it. Then:

```nix
# Copied from nixpkgs PR #NNNNN (version X, head <sha>). Delete
# pkgs/<attr>/ along with this file once the channel ships it.
{ final, ... }: final.callPackage ../../pkgs/<attr>/package.nix { }
```

`callPackage` resolves against `final`, so related bumps see each other, as
`herdr-desktop` gets the bumped `herdr-core`. Do **not** also export it from
`pkgs/default.nix` under the same name: the `additions` overlay would bypass
the fallback. `pkgs/code-cursor` is exported as `cursor` for exactly this
reason.

**C. From-scratch derivation.** Use this for an upstream release binary when
nixpkgs builds from source and can't keep up. Write a `stdenvNoCC.mkDerivation`
with `inherit (prev.<attr>) meta` (or `meta = prev.<attr>.meta // { platforms = ...; }`).
Set `meta.platforms` to the systems you actually have hashes for: platform
fallback relies on it, and an unsupported system then quietly gets nixpkgs'
build instead of failing on a missing hash. For a worked example, see the
retired codex release-binary override (multi-platform hashes, wrapper):
`git show 9446be6:overlays/default.nix`.

**Ties.** Set `passthru.preferOnTie = true` only when nixpkgs' build of the
*same* version is broken. Say why in the comment.

**Version strings.** The comparison is `builtins.compareVersions`. Use the
version scheme nixpkgs uses for the attribute (`0-unstable-2026-11-01` style
for git snapshots), or ours may never count as newer, or may count as newer
forever. Keep `version` a literal or derived from cheap values: it is
evaluated on every host that uses the attribute.

## 3. Hashes

Put `hash = lib.fakeHash;` in the file, build, and copy the `got:` hash from the
error. Repeat for each fixed-output derivation (src first, then deps). For
release assets, `nix-prefetch-url <url>` or the project's published checksums
(convert with `nix hash convert --hash-algo sha256 <hex>`) avoid a build.
Prefetch every platform listed in `meta.platforms`, not only this machine's.

## 4. Verify

```
git add overlays/bumps/<attr>.nix pkgs/<attr>     # flakes only see tracked files
nix eval --raw .#nixosConfigurations.<host>.pkgs.<attr>.version   # ours, no "obsolete" warning
just build-host <host>                            # or nix build .#nixosConfigurations.<host>.pkgs.<attr>
nixcfg check-host --all
```

Also run the binary once (`<result>/bin/<name> --version`) if the package has
no `versionCheckHook`.

A from-source bump of something heavy that romeo-2 or whiskey-1 uses is no
longer in the binary cache. Pre-seed it (`nixcfg preseed romeo-2`, see
`deploy-change`), or every host compiles it.

## 5. Ship

Commit with a pathspec that includes any vendored files:
`git commit -m "feat(pkgs): bump <attr> to X ahead of nixpkgs" -- overlays/bumps/<attr>.nix pkgs/<attr>`,
then follow `deploy-change`. `overlays/` is `**.nix`, so CI runs.

## Removing a bump

When the `overlays/bumps/<attr>.nix is obsolete` warning appears (in CI logs,
`nix eval`, or rebuilds), nixpkgs has caught up and the attribute is already
nixpkgs' own. Delete the file and any `pkgs/<attr>/` it called. Deleting it
does not change any derivation. If the bump had `preferOnTie`, first check
that nixpkgs' version actually builds (`just build-host <host>`). Bumping
further ahead instead means editing the version and hashes in place.
