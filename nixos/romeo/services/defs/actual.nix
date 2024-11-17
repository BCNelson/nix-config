{ dataDirs }:
{
  actual_server = {
    image = "ghcr.io/actualbudget/actual-server:latest";
    container_name = "actual-server";
    ports = ["127.0.0.1:5006:5006"];
    volumes = [
      "${dataDirs.level3}/actual:/data"
    ];
    restart = "unless-stopped";
  };
}
