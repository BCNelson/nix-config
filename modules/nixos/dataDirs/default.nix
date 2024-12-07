{ lib, ... }:
{
  options = {
    data.dirs = {
      level1 = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "The level 1 data directory";
      };
      level2 = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "The level 2 data directory";
      };
      level3 = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "The level 3 data directory";
      };
      level4 = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "The level 4 data directory";
      };
      level5 = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "The level 5 data directory";
      };
      level6 = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "The level 6 data directory";
      };
      level7 = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "The level 7 data directory";
      };
    };
  };
}
