{ config, lib, ... }:

let

  cfg = config.services.freedesktop.autostart;

in
{
  meta.maintainers = [ lib.maintainers.lheckemann ];

  options = {
    services.freedesktop.autostart = {
      enable = lib.mkEnableOption "Enable autostarting of applications";
      packages = lib.mkOption {
        type = lib.types.listOf lib.types.package;
        default = [ ];
        description = "List of packages to autostart";
      };
      desktopItems = lib.mkOption {
        type = lib.types.listOf lib.types.attrs;
        default = [ ];
        description = "List of desktopItems to autostart";
      };
      packageSourced = lib.mkOption {
        type = lib.types.listOf lib.types.attrs;
        default = [ ];
        description = "List of packages to autostart from source";
      };
    };
  };

  config = lib.mkIf cfg.enable {
    home.file = lib.mkMerge [
      (builtins.listToAttrs (map
        (pkg: {
          name = # ".config/autostart/" + pkg.pname + ".desktop";
            if pkg ? desktopItem then ".config/autostart/" + pkg.desktopItem.name else ".config/autostart/" + pkg.pname + ".desktop";
          value =
            if pkg ? desktopItem then {
              # Application has a desktopItem entry. 
              # Assume that it was made with makeDesktopEntry, which exposes a
              # text attribute with the contents of the .desktop file
              inherit (pkg.desktopItem) text;
            } else {
              # Application does *not* have a desktopItem entry. Try to find a
              # matching .desktop name in /share/applications
              source = pkg + "/share/applications/" + pkg.pname + ".desktop";
            };
        })
        cfg.packages))
      (builtins.listToAttrs (map
        (item: {
          name = ".config/autostart/" + item.name;
          value = item.text;
        })
        cfg.desktopItems))
      (builtins.listToAttrs (map
        (item: {
          name = ".config/autostart/" + item.package.pname + ".desktop";
          value = {
            source = item.package + "/" + item.path;
          };
        })
        cfg.packageSourced))
    ];
  };
}
