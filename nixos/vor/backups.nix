_: 
{
    services.sanoid = {
        enable = true;
        datasets = {
            "liveData/NelsonData" = {
                hourly = 72;
                daily = 31;
                weekly = 52;
                monthly = 24;
                yearly = 10;
                useTemplate = [ "common" ];
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
}