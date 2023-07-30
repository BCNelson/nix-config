{ config, pkgs, ... }:

{
    home.packages = [
        pkg.deckmaster
    ];

    home.file."${config.xdg.configHome}/deckmaster" = {
        source = ./files;
        recursive = true;
    };

    systemd.user = {
        paths = {
            deckmaster = {
                enable = true;
                Unit.Description = "Stream Deck Device Path";
                Path = {
                    PathExists = "/dev/streamdeck";
                    Unit = "deckmaster.service";
                };
                Install.WantedBy = [ "multi-user.target" ];
            };
        };
        services = {
            deckmaster = {
                enable = true;
                Unit.Description = "Deckmaster Service";
                Service = {
                    ExecStart= "${pkg.deckmaster}/bin/deckmaster --deck ${config.xdg.configHome}/deckmaster/main.deck";
                    Restart = "on-failure";
                    ExecReload = "kill -HUP $MAINPID";
                }
            };
        };
    }
}