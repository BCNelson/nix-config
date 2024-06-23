{ dataDirs, libx }:
let
  tubearchivist_ELASTIC_PASSWORD = libx.getSecret ../../sensitive.nix "tubearchivist_ELASTIC_PASSWORD";
  tubearchivist_password = libx.getSecret ../../sensitive.nix "tubearchivist_password";
in
{
  tubearchivist = {
    image = "bbilly1/tubearchivist:lastest";
    container_name = "tubearchivist";
    user = "1000:1000";
    volumes = [
      "${dataDirs.level6}/media/youtube:/youtube"
      "${dataDirs.level7}/tubearchivist:/cache"
    ];
    restart = "unless-stopped";
    environment = [
      "ES_URL=http://archivist-es:9200"
      "REDIS_HOST=archivist-redis"
      "HOST_UID=1000"
      "HOST_GID=1000"
      "TA_HOST=tube.romeo.b.nel.family"
      "TA_USERNAME=tubearchivist"
      "TA_PASSWORD=${tubearchivist_password}"
      "ELASTIC_PASSWORD=${tubearchivist_ELASTIC_PASSWORD}"
      "TZ=America/Denver"
    ];
    healthcheck = {
      test = [ "CMD" "curl" "-f" "http://localhost:8000/health" ];
      interval = "2m";
      timeout = "10s";
      retries = 3;
      start_period = "30s";
    };
    depends_on = [ "archivist-es" "archivist-redis" ];
  };
  archivist-redis = {
    image = "redis/redis-stack-server";
    container_name = "archivist-redis";
    restart = "unless-stopped";
    volumes = [ "${dataDirs.level6}/archivistRedis:/data" ];
    depends_on = [ "archivist-es" ];
  };
  archivist-es = {
    image = "bbilly1/tubearchivist-es";
    container_name = "archivist-es";
    restart = "unless-stopped";
    environment = [
      "ELASTIC_PASSWORD=${tubearchivist_ELASTIC_PASSWORD}"
      "ES_JAVA_OPTS=-Xms512m -Xmx512m"
      "xpack.security.enabled=true"
      "discovery.type=single-node"
      "path.repo=/usr/share/elasticsearch/data/snapshot"
    ];
    ulimits = {
      memlock = {
        soft = -1;
        hard = -1;
      };
    };
    volumes = [ "${dataDirs.level6}/archivistEs:/usr/share/elasticsearch/data" ];
  };
}
