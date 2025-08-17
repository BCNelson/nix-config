{ lib, ... }:

with lib;

let
  uriType = types.submodule {
    options = {
      uri = mkOption {
        type = types.str;
        description = "The URI for the login item";
      };
      matchType = mkOption {
        type = types.enum [ "domain" "host" "startsWith" "exact" "regex" "never" ];
        default = "domain";
        description = "How Bitwarden should match this URI";
      };
    };
  };

  fieldType = types.submodule {
    options = {
      name = mkOption {
        type = types.str;
        description = "Name of the custom field";
      };
      value = mkOption {
        type = types.str;
        description = "Value of the custom field";
      };
      type = mkOption {
        type = types.enum [ "text" "hidden" "boolean" ];
        default = "text";
        description = "Type of the custom field";
      };
    };
  };
in
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
            uris = mkOption {
              type = types.nullOr (types.oneOf [
                types.str
                (types.listOf (types.oneOf [
                  types.str
                  uriType
                ]))
              ]);
              default = null;
              description = "URIs for login items. Can be a string, list of strings, or list of URI objects with matchType";
            };
            username = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "Username for login items (if set, creates a login item instead of secure note)";
            };
            folder = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "Folder to place the item in (default: 'NixOS Secrets')";
            };
            favorite = mkOption {
              type = types.bool;
              default = false;
              description = "Mark the item as a favorite";
            };
            reprompt = mkOption {
              type = types.bool;
              default = false;
              description = "Require master password re-prompt to view";
            };
            notes = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "Additional notes for the item";
            };
            totp = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "TOTP seed for two-factor authentication";
            };
            fields = mkOption {
              type = types.nullOr (types.listOf fieldType);
              default = null;
              description = "Custom fields for the item";
            };
            organizationId = mkOption {
              type = types.nullOr types.str;
              default = null;
              description = "Organization ID to assign the item to";
            };
            collectionIds = mkOption {
              type = types.nullOr (types.listOf types.str);
              default = null;
              description = "Collection IDs to add the item to";
            };
          };
        });
        default = null;
        description = "Bitwarden sync configuration for this secret";
      };
    }));
  };
}