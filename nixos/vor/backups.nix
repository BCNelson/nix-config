_: 
{
    services.sanoid = {
        enable = true;
        datasets = {
            "liveData/NelsonData" = {
                hourly = 72;
                daily = 31;
                weekly = 26;
                monthly = 12;
                yearly = 5;
                useTemplate = [ "common" ];
            };
            "vault/Backups/Nelson Family Data" = {
                hourly = 72;
                daily = 31;
                weekly = 52;
                monthly = 24;
                yearly = 10;
                useTemplate = [ "common" ];
                autosnap = false;
            };
        };
        templates = {
            "common" = {
                autoprune = true;
                autosnap = true;
                recursive = true;
            };
        };
    };
    services.syncoid = {
        enable = true;
        commands = {
            "liveData/NelsonData Local Backup" = {
                source = "liveData/NelsonData";
                target = "vault/Backups/Nelson Family Data";
            };
        };
    };
}