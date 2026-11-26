# Version bumps: packages we carry ahead of nixpkgs.
#
# One file per package, named after its attribute: ./<attr>.nix. The file is
# `{ final, prev, inputs, lib }: <derivation>` and returns our build -- an
# `prev.<attr>.overrideAttrs` version/src bump, a `final.callPackage` of a
# vendored package.nix, or a from-scratch derivation. Nothing else to wire up;
# every file in this directory is picked up.
#
# The rule, applied to all of them the same way:
#
#   - ours is used only while it is strictly newer than nixpkgs' `prev.<attr>`
#     and supports the host platform (`meta.platforms` / `badPlatforms`);
#     otherwise the attribute is nixpkgs' own, untouched, with its binary cache.
#   - a tie goes to nixpkgs, unless our build sets `passthru.preferOnTie = true`
#     (for when nixpkgs' build of the same version is broken).
#   - once nixpkgs has caught up, evaluating the attribute warns that the bump
#     file is dead and should be deleted (with any vendored files it points at).
#
# Only `version` and `meta` of our build are read to decide, so a bump that
# has been overtaken is never fetched or built.
#
# Fixes and wrappers that should apply whichever version wins (e.g. the
# claude-code PATH wrapper) belong in `modifications`, which sees the result
# of this overlay as `prev`.
{ inputs }:
final: prev:
let
  # prev, not final: the overlay's attribute names come from lib, and taking
  # them from final would make them depend on this overlay's own result.
  inherit (prev) lib;

  files = lib.filterAttrs (
    file: type: type == "regular" && file != "default.nix" && lib.hasSuffix ".nix" file
  ) (builtins.readDir ./.);

  pick =
    name: ours:
    let
      upstream = prev.${name} or null;
      upstreamVersion = if upstream == null then "0" else upstream.version or "0";
      cmp = builtins.compareVersions ours.version upstreamVersion;
      newer = cmp > 0 || (cmp == 0 && ours.preferOnTie or false);
      supported = lib.meta.availableOn final.stdenv.hostPlatform ours;
    in
    if newer && supported then
      ours
    else if !newer then
      lib.warn "overlays/bumps/${name}.nix is obsolete: nixpkgs has ${upstreamVersion} (ours: ${ours.version}). Delete it." upstream
    else
      upstream;
in
lib.mapAttrs' (
  file: _:
  let
    name = lib.removeSuffix ".nix" file;
  in
  lib.nameValuePair name (
    pick name (
      import (./. + "/${file}") {
        inherit final prev inputs lib;
      }
    )
  )
) files
