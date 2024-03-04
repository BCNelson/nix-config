{ dataDirs, libx }:
let
  smtp_password = libx.getSecret ../../../sensitive.nix "smtp_password";
in
{
  mealie = {
    image = "ghcr.io/mealie-recipes/mealie:v1.2.0";
    container_name = "mealie";
    volumes = [
      "${dataDirs.level2}/mealie:/app/data"
    ];
    environment = [
      "ALLOW_SIGNUP=false"
      "PUID=1000"
      "PGID=1000"
      "TZ=America/Denver"
      "WEB_GUNICORN=false"
      "MAX_WORKERS=1"
      "WEB_CONCURRENCY=1"
      "BASE_URL=https://recipes.nel.family"
      "SMTP_HOST=smtp.migadu.com"
      "SMTP_PORT=465"
      "SMTP_FROM_NAME=Nelson Family Admin"
      "SMTP_AUTH_STRATEGY=SSL"
      "SMTP_FROM_EMAIL=admin@nel.family"
      "SMTP_PASSWORD=${smtp_password}"
    ];
    restart = "unless-stopped";
  };
}
