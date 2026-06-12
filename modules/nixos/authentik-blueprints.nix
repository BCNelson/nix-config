# Companion module extending nix-community/authentik-nix with a first-class
# option for merging custom blueprints into authentik's blueprints_dir.
#
# Why this is needed: authentik's blueprint loader resolves each blueprint path
# (authentik/blueprints/models.py retrieve_file) and rejects it unless the
# resolved path still lives under blueprints_dir. A symlinked tree (e.g.
# pkgs.symlinkJoin) resolves out to other /nix/store paths and fails with
# "Invalid blueprint path", which crash-loops the core on system/bootstrap.yaml.
# The upstream module only exposes blueprints_dir (a single real dir) with no
# merge mechanism, so every consumer hand-rolls one and hits this footgun.
#
# This option copies (dereferencing symlinks) the package's bundled blueprints
# plus any extra directories into one real-file directory and points
# blueprints_dir at it. Candidate for upstreaming to authentik-nix.
{ config, options, lib, pkgs, ... }:

with lib;

let
  cfg = config.services.authentik;

  # Raw `settings` definitions that assign blueprints_dir. Upstream sets it with
  # mkDefault and we set it with mkForce below; both are override-wrapped
  # (`{ _type = "override"; ... }`). A plain (string) assignment is therefore a
  # user-provided blueprints_dir, which conflicts with extraBlueprints.
  blueprintsDirDefs = filter (d: d.value ? blueprints_dir)
    options.services.authentik.settings.definitionsWithLocations;
  userBlueprintsDirDefs = filter
    (d: !(isAttrs d.value.blueprints_dir && (d.value.blueprints_dir._type or null) == "override"))
    blueprintsDirDefs;
in
{
  options.services.authentik.extraBlueprints = mkOption {
    type = types.listOf types.path;
    default = [ ];
    example = literalExpression "[ ./authentik/blueprints ]";
    description = ''
      Extra blueprint directories to merge into authentik's blueprints_dir, on
      top of the package's bundled blueprints. Later entries win on conflict.

      Contents are copied with symlinks dereferenced into a single real-file
      directory, because authentik rejects symlinked blueprint paths. Setting
      this manages {option}`services.authentik.settings.blueprints_dir` for you,
      so the two are mutually exclusive.
    '';
  };

  config = mkIf (cfg.enable && cfg.extraBlueprints != [ ]) {
    assertions = [
      {
        assertion = userBlueprintsDirDefs == [ ];
        message = ''
          services.authentik: set either `extraBlueprints` or
          `settings.blueprints_dir`, not both — `extraBlueprints` manages
          `blueprints_dir` for you. Remove the manual `blueprints_dir` set in:
          ${concatMapStringsSep "\n  " (d: toString d.file) userBlueprintsDirDefs}
        '';
      }
    ];

    services.authentik.settings.blueprints_dir = mkForce (
      pkgs.runCommand "authentik-blueprints" { } ''
        mkdir -p "$out"
        cp -rL ${cfg.authentikComponents.staticWorkdirDeps}/blueprints/. "$out"/
        ${concatMapStringsSep "\n" (d: ''cp -rL ${d}/. "$out"/'') cfg.extraBlueprints}
        chmod -R u+w "$out"
      ''
    );
  };
}
