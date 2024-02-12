{ dataDirs }:
{
  actual-server = {
    image = "docker.io/actualbudget/actual-server:latest-alpine";
    volumes = [ "${dataDirs.level2}/actualBudget:/data" ];
    restart = "unless-stopped";
  };
}
