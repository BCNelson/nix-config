{ dataDirs, libx, pkgs }:
let
  sensitiveData = libx.getSecret ../../../sensitive.nix;
  config = pkgs.writeText "config.yaml" (builtins.toJSON {
    service = {
      timezone = "America/Denver";
    };
    cache = {
      enabled = false;
    };
    cors = {
      enabled = true;
      allowed_origins = [ "*" ];
    };
    mailer = {
      enabled = true;
      host = "smtp.migadu.com";
      port = 465;
      username = "admin@nel.family";
      password = "${sensitiveData "smtp_password"}";
      skiptlsverify = false;
      fromemail = "admin@nel.family";
      queuelength = 100;
      queuetimeout = 30;
      forcessl = true;
    };
    log = {
      enabled = true;
      path = "<rootpath>logs";
      standard = "stdout";
      level = "INFO";
      database = "off";
      databaselevel = "WARNING";
      http = "stdout";
      echo = "off";
      events = "stdout";
      eventslevel = "INFO";
    };
    ratelimit = {
      enabled = false;
    };
    files = {
      enabled = true;
      path = "./files";
      maxsize = "50MB";
    };
    avatar = {
      gravatarexpiration = 3600;
    };
    backgrounds = {
      enabled = true;
      providers = {
        upload = {
          enabled = true;
        };
        unsplash = {
          enabled = false;
        };
      };
    };
    keyvalue = {
      type = "memory";
    };
    metrics = {
      enabled = false;
    };
    defaultsettings = {
      avatar_provider = "initials";
      avatar_file_id = 0;
      email_reminders_enabled = false;
      discoverable_by_name = true;
      discoverable_by_email = true;
      overdue_tasks_reminders_enabled = true;
      overdue_tasks_reminders_time = "9:00";
      default_list_id = 0;
      week_start = 0;
      language = "en";
    };
    auth = {
      local = {
        enabled = true;
      };
      openid = {
        enabled = true;
        redirecturl = "https://todo.nel.family/auth/openid/";
        providers = [
          {
            name = "Kanidm";
            authurl = "https://idm.nel.family/oauth2/openid/vikunja";
            clientid = "vikunja";
            clientsecret = "${sensitiveData "kanidm_vikunja_client_secret"}";
          }
        ];
      };
    };
  });
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
  vikunja = {
    image = "vikunja/vikunja";
    environment = [
      "VIKUNJA_DATABASE_HOST=vikunja_db"
      "VIKUNJA_DATABASE_PASSWORD=${sensitiveData "vikunja_postgress_password"}"
      "VIKUNJA_DATABASE_TYPE=postgres"
      "VIKUNJA_DATABASE_USER=vikunja"
      "VIKUNJA_DATABASE_DATABASE=vikunja"
      "VIKUNJA_SERVICE_JWTSECRET=${sensitiveData "vikunja_jwt_secret" }"
      "VIKUNJA_SERVICE_PUBLICURL=https://todo.nel.family/"
    ];
    volumes = [
      "${dataDirs.level5}/vikunja/files:/app/vikunja/files"
      "${config}:/app/vikunja/config.yml"
    ];
    depends_on = [ "vikunja_db" ];
    ports = [ "3456:3456" ];
    restart = "unless-stopped";
  };
}
