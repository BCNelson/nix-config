{ dataDirs, libx }:
let
  sensitiveData = libx.getSecret ../../../sensitive.nix;
  config = ".";
in
{
  vikunja_db = {
    image = "postgres:13";
    environment = [
      "POSTGRES_PASSWORD=${sensitiveData "vikunja_postgress_password"}"
      "POSTGRES_USER=vikunja"
    ];
    volumes = [
      "${dataDirs.level5}/vikunja/database:/var/lib/postgresql/data"
    ];
    restart = "unless-stopped";
  };
  vikunja_api = {
    image = "vikunja/api";
    environment = [
      "VIKUNJA_DATABASE_HOST=vikunja_db"
      "VIKUNJA_DATABASE_PASSWORD=${sensitiveData "vikunja_postgress_password"}"
      "VIKUNJA_DATABASE_TYPE=postgres"
      "VIKUNJA_DATABASE_USER=vikunja"
      "VIKUNJA_DATABASE_DATABASE=vikunja"
      "VIKUNJA_SERVICE_JWTSECRET=${sensitiveData "vikunja_jwt_secret" }"
      "VIKUNJA_SERVICE_FRONTENDURL=https://todo.nel.family/"
    ];
    volumes = [
      "${dataDirs.level5}/vikunja/files:/app/vikunja/files"
      "${config}/vikunja/config.yml:/app/vikunja/config.yml"
    ];
    depends_on = [ "vikunja_db" ];
    restart = "unless-stopped";
  };
  vikunja_frontend = {
    image = "vikunja/frontend";
    environment = [
      "VIKUNJA_API_URL=https://todo.nel.family/api/v1"
    ];
    restart = "unless-stopped";
  };
}
