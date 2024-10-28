{ config, lib, pkgs, ... }:

let

  cfg = config.services.bcnelson.recovery;

in
{
  options = {
    services.bcnelson.recovery = {
      enable = lib.mkEnableOption "Enable Recovery environment";
      hardwareconfig = lib.mkOption {
        type = lib.types.nullOr lib.types.path;
        default = null;
        description = "The hardware config that should be used";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    specialisation.recovery = {
      inheritParentConfig = false;
      configuration = {
        imports = [
          cfg.hardwareconfig
        ];
        users.users.recovery = {
          isNormalUser = true;
          home = "/home/recovery";
          description = "Recovery user";
          extraGroups = [ "wheel" ];
        };
      };
    };
  };
}
