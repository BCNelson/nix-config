{ libx }:
let
  jwt_secret = libx.getSecret ./sensitive.nix "jwt_secret";
  hmac_secret = libx.getSecret ./sensitive.nix "hmac_secret";
  oidc_issuer_private_key = libx.getSecret ./sensitive.nix "oidc_issuer_private_key";
  database_key = libx.getSecret ./sensitive.nix "database_key";
  smtp_password = libx.getSecret ../../../../sensitive.nix "smtp_password";
in
{
  server = {
    host = "0.0.0.0";
    port = 9091;
    read_buffer_size = 4096;
    write_buffer_size = 4096;
    path = "authelia";
  };

  theme = "auto";

  log = {
    # Level of verbosity for logs: info, debug, trace
    level = "info";
    file_path = "/config/authelia.log";
  };

  # # JWT Secret can also be set using a secret: https://docs.authelia.com/configuration/secrets.html
  inherit jwt_secret;

  totp = {
    issuer = "h.b.nel.family";
    period = 30;
    skew = 1;
  };

  oidc = {
    inherit hmac_secret;
    issuer_private_key = oidc_issuer_private_key;
    access_token_lifespan = "1h";
    authorize_code_lifespan = "1m";
    id_token_lifespan = "1h";
    refresh_token_lifespan = "90m";
    enable_client_debug_messages = true;
    clients = [ ];
  };

  authentication_backend = {
    disable_reset_password = false;
    file = {
      path = "/config/users_database.yml";
      password = {
        algorithm = "argon2id";
        iterations = 1;
        key_length = 32;
        salt_length = 16;
        memory = 512;
        parallelism = 8;
      };
    };
    access_control = {
      default_policy = "deny";
      networks = [ ];
      rules = [ ];
    };
  };

  session = {
    name = "authelia_session";
    expiration = "1h";
    inactivity = "5m";
    remember_me_duration = "1M";
    domain = "h.b.nel.family";
  };

  regulation = {
    max_retries = 3;
    find_time = "2m";
    ban_time = "5m";
  };

  storage = {
    encryption_key = database_key;
    local = {
      path = "/config/db.sqlite3";
    };
  };

  notifier = {
    disable_startup_check = false;
    smtp = {
      username = "admin@nel.family";
      password = smtp_password;
      host = "smtp.migadu.com";
      port = 465;
      sender = "admin@nel.family";
      subject = "[Authelia] {title}";
      startup_check_address = "test@authelia.com";
    };
  };

}
