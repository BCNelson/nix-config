{ lib, ... }:

with lib;

{
  options.age.secrets = mkOption {
    type = types.attrsOf (types.submodule ({ ... }: {
      options.bitwarden = mkOption {
        type = types.nullOr (types.submodule {
          options = {
            name = mkOption {
              type = types.str;
              description = "Name of the item in Bitwarden";
            };
            url = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "URL associated with the secret (for login items)";
            };
            username = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "Username for login items (if set, creates a login item instead of secure note)";
            };
          };
        });
        default = null;
        description = "Bitwarden sync configuration for this secret";
      };
    }));
  };
}