{ config, lib, ... }:

let
  thinClients = import ../../../hosts/thin-clients.nix;

  # Name baked into every signature, so it must match the public key clients
  # trust. Changing it means re-signing every published closure.
  keyName = "nixcache.nel.family-1";
in
{
  config = lib.mkMerge [
    {
      # The key the builder signs published closures with. agenix generates it
      # (`just generate-secrets`), encrypted to the master identities --
      # encryption needs only their public halves, so that step wants no
      # hardware key; `just rekey` still does.
      #
      # The generator also drops the public half next to the .age file. Clients
      # have to trust that key at *evaluation* time, so it has to be committed
      # in the clear -- this is the same adjacent-file trick agenix-rekey's own
      # wireguard example uses.
      age.secrets.nix_cache_key = {
        rekeyFile = ../../../secrets/store/romeo/nix_cache_key.age;
        generator = {
          tags = [ "nix-cache" ];
          script = { pkgs, lib, file, ... }: ''
            priv=$(${pkgs.nix}/bin/nix key generate-secret \
              --extra-experimental-features nix-command --key-name ${keyName})
            ${pkgs.nix}/bin/nix key convert-secret-to-public \
              --extra-experimental-features nix-command <<< "$priv" \
              > ${lib.escapeShellArg (lib.removeSuffix ".age" file + ".pub")}
            echo "$priv"
          '';
        };
      };
    }

    (lib.mkIf (thinClients != [ ]) {
      services.bcnelson.thinClientBuilder = {
        enable = true;
        hosts = thinClients;
        # The same checkout auto-update rebuilds this machine from, so a closure
        # a thin client can see always comes from a commit romeo itself has
        # already built successfully.
        configPath = config.services.bcnelson.autoUpdate.path;
        signingKeyFile = config.age.secrets.nix_cache_key.path;
        # Document root of the binary cache vhost, so anything copied here is
        # immediately fetchable at https://<domain>/.
        cacheDir = "/var/public-nix-cache";
        inherit (config.services.bcnelson.binary-cache-proxy) domain;
      };
    })
  ];
}
