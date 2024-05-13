{ dataDirs, libx }:
let
  healthchecks_admin_password = libx.getSecret ../../sensitive.nix "healthchecks_admin_password";
  healthchecks_secret_key = libx.getSecret ../../sensitive.nix "healthchecks_secret_key";
in
{
  healthchecks = {
    image = "ghcr.io/linuxserver/healthchecks";
    container_name = "healthchecks";
    environment = [
      "PUID=1000"
      "PGID=1000"
      "SITE_ROOT=https://health.b.nel.family"
      "SITE_NAME=BHNelson Family Health Checks"
      "ALLOWED_HOSTS=https://health.b.nel.family"
      "SUPERUSER_EMAIL=bradley@nel.family"
      "SUPERUSER_PASSWORD=${healthchecks_admin_password}"
      "REGENERATE_SETTINGS=False"
      "SECRET_KEY=${healthchecks_secret_key}"
    ];
    volumes = [
      "${dataDirs.level3}/healthchecks:/config"
    ];
    ports = [
      "127.0.0.1:8000:8000"
    ];
    restart = "unless-stopped";
  };
}
