{ dataDirs }:
let
  sensitiveData = import ../../../sensitive.nix;
in
{
  mealie-frontend = {
    image = "hkotel/mealie:frontend-v1.0.0beta-5";
    container_name = "mealie-frontend";
    environment = [
      "API_URL=http://mealie-api:9000"
      "ALLOW_SIGNUP=false"
    ];
    volumes = [
      "${dataDirs.level2}/mealie:/app/data"
    ];
    restart = "unless-stopped";
    depends_on = [ "mealie-api" ];
  };
  mealie-api = {
    image = "hkotel/mealie:api-v1.0.0beta-5";
    container_name = "mealie-api";
    volumes = [
      "${dataDirs.level2}/mealie:/app/data"
    ];
    environment = [
      "ALLOW_SIGNUP=false"
      "PUID=1000"
      "PGID=1000"
      "TZ=America/Denver"
      "MAX_WORKERS=1"
      "WEB_CONCURRENCY=1"
      "BASE_URL=https://recipes.nel.family"
      "SMTP_HOST=smtp.migadu.com"
      "SMTP_PORT=465"
      "SMTP_FROM_NAME=Nelson Family Admin"
      "SMTP_AUTH_STRATEGY=SSL"
      "SMTP_FROM_EMAIL=admin@nel.family"
      "SMTP_PASSWORD=${sensitiveData.smtp_password}"
    ];
    restart = "unless-stopped";
  };
}
