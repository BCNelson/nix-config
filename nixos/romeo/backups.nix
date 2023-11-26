_: 
{
    services.sanoid = {
        enable = true;
        datasets = {
            "vault/data/level1" = {
                hourly = 72;
                daily = 31;
                weekly = 52;
                monthly = 24;
                yearly = 10;
                useTemplate = [ "common" ];
            };
            "vault/data/level2" = {
                hourly = 72;
                daily = 31;
                weekly = 52;
                monthly = 24;
                yearly = 10;
                useTemplate = [ "common" ];
            };
            "vault/data/level3" = {
                hourly = 72;
                daily = 31;
                weekly = 24;
                monthly = 12;
                yearly = 2;
                useTemplate = [ "common" ];
            };
            "vault/data/level4" = {
                hourly = 72;
                daily = 31;
                weekly = 24;
                monthly = 12;
                yearly = 2;
                useTemplate = [ "common" ];
            };
            "vault/data/level5" = {
                hourly = 72;
                daily = 31;
                weekly = 24;
                monthly = 12;
                yearly = 2;
                useTemplate = [ "common" ];
            };
            "scary/replaceable" = {
                hourly = 24;
                daily = 31;
                weekly = 8;
                monthly = 6;
                yearly = 1;
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