{ dataDirs }:
{
  fastenhealth = {
    image = "ghcr.io/fastenhealth/fasten-onprem:main";
    container_name = "fastenhealth";
    volumes = [
      "${dataDirs.level3}/fastenhealth/db:/opt/fasten/db"
      "${dataDirs.level7}/fastenhealth/cache:/opt/fasten/cache"
    ];
    ports = [ "8081:8080" ];
  };
}
