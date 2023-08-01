{ config, pkgs, ... }:

{
    home.packages = [
        pkgs.deckmaster
        pkgs.wl-clipboard
    ];

    home.file."${config.xdg.configHome}/deckmaster" = {
        source = ./files;
        recursive = true;
        onChange = "systemctl --user restart deckmaster.service";
    };

    systemd.user = {
        paths = {
            deckmaster = {
                Unit.Description = "Stream Deck Device Path";
                Path = {
                    PathExists = "/dev/streamdeck";
                    Unit = "deckmaster.service";
                };
                Install.WantedBy = [ "default.target" ];
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