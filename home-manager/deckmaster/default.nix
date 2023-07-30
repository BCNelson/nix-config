{ config, pkgs, ... }:

{
    home.packages = [
        pkgs.deckmaster
    ];

    home.file."${config.xdg.configHome}/deckmaster" = {
        source = ./files;
        recursive = true;
    };

    systemd.user = {
        paths = {
            deckmaster = {
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
                Unit.Description = "Deckmaster Service";
                Service = {
                    ExecStart= "${pkgs.deckmaster}/bin/deckmaster --deck ${config.xdg.configHome}/deckmaster/main.deck";
                    Restart = "on-failure";
                    ExecReload = "kill -HUP $MAINPID";
                };
            };
        };
    };
}