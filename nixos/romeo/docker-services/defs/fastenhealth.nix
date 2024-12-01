{ dataDirs }:
{
  fastenhealth = {
    image = "ghcr.io/fastenhealth/fasten-onprem:main";
    container_name = "fastenhealth";
    volumes = [
      "${dataDirs.level3}/fastenhealth/db:/opt/fasten/db"
      "${dataDirs.level7}/fastenhealth/cache:/opt/fasten/cache"
    ];
    ports = [ "127.0.0.1:8081:8080" ];
  };
}
