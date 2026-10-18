{ config, lib, pkgs, ... }:

let

  cfg = config.services.bcnelson.generationRetention;
  pruneScript = pkgs.writeShellApplication {
    name = "prune-generations";
    runtimeInputs = with pkgs; [ coreutils findutils config.nix.package ];
    text = builtins.readFile ./prune-generations.sh;
  };

in
{
  options = {
    services.bcnelson.generationRetention = {
      enable = lib.mkEnableOption ''
        generation expiry ahead of nix-gc that never deletes the current or
        booted system generation. Replaces `nix.gc.options =
        "--delete-older-than ..."`, which can't exclude a generation and so
        eventually deletes the booted one on long-running hosts
      '';
      olderThanDays = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.positive;
        default = 7;
        description = "Delete profile generations older than this many days. null disables age-based expiry.";
      };
      keep = lib.mkOption {
        type = lib.types.nullOr lib.types.ints.positive;
        default = null;
        description = "Keep at most this many newest system generations (the current and booted ones are always kept on top). null disables count-based expiry.";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [{
      assertion = !(lib.hasInfix "--delete-old" config.nix.gc.options || lib.hasInfix " -d" (" " + config.nix.gc.options));
      message = "nix.gc.options deletes generations itself, which defeats services.bcnelson.generationRetention; set olderThanDays/keep instead.";
    }];

    systemd.services.prune-generations = {
      description = "Expire old Nix profile generations, keeping the current and booted system";
      requiredBy = [ "nix-gc.service" ];
      before = [ "nix-gc.service" ];
      environment = {
        OLDER_THAN_DAYS = lib.optionalString (cfg.olderThanDays != null) (toString cfg.olderThanDays);
        KEEP = lib.optionalString (cfg.keep != null) (toString cfg.keep);
      };
      serviceConfig = {
        Type = "oneshot";
        ExecStart = lib.getExe pruneScript;
      };
    };
  };
}
