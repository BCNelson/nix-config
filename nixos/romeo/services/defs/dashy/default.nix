{ pkgs }:
let
  dashyConfig = pkgs.writeTextFile {
    name = "dashy-config";
    text = builtins.readFile ./config.yml;
    destination = "/config.yml";
  };
in
{
  dashy = {
    image = "lissy93/dashy";
    container_name = "Dashy";
    environment = [
      "NODE_ENV=production"
    ];
    restart = "unless-stopped";
    volumes = [
      "${dashyConfig}/config.yml:/public/conf.yml:ro"
    ];
    healthcheck = {
      test = [ "CMD" "node" "/app/services/healthcheck" ];
      interval = "1m30s";
      timeout = "10s";
      retries = 3;
      start_period = "40s";
    };
  };
}
