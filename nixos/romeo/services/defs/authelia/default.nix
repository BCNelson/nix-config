{ dataDirs, pkgs, libx }:
let
  autheliaConfig = pkgs.writeTextFile {
    name = "authelia-config.yml";
    text = builtins.toJSON (import ./config.nix { inherit libx;});
    destination = "/config.yml";
  };
  authelia-users = pkgs.writeTextFile {
    name = "authelia-config.yml";
    text = builtins.toJSON (libx.getSecret ./sensitive.nix "users");
    destination = "/users.yml";
  };
in
{
  authelia = {
    image = "authelia/authelia";
    container_name = "authelia";
    environment = {
      TZ = "America/Denver";
    };
    volumes = [
      "${autheliaConfig}/config.yml:/config/configuration.yml"
      "${authelia-users}/users.yml:/config/users_database.yml"
      "${dataDirs.level1}/authelia/db.sqlite3:/config/db.sqlite3"
    ];
  };
}
