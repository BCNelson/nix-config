{ config, pkgs, ... }:
let
  dataDirs = config.data.dirs;

  # Minimal Node-RED settings that turns on the editor login (adminAuth) and
  # pins a credentialSecret so flow credentials survive restarts/redeploys.
  # The actual values come from the environment; see node-red-admin-env below.
  # Any key we omit falls back to Node-RED's built-in defaults.
  settingsJs = pkgs.writeText "node-red-settings.js" ''
    module.exports = {
        flowFile: 'flows.json',
        credentialSecret: process.env.NODE_RED_CREDENTIAL_SECRET,
        adminAuth: {
            type: "credentials",
            users: [
                {
                    username: process.env.NODE_RED_ADMIN_USER || "admin",
                    password: process.env.NODE_RED_ADMIN_PASSWORD_HASH,
                    permissions: "*"
                }
            ]
        },
        uiPort: process.env.PORT || 1880,
        functionGlobalContext: {},
        exportGlobalContextKeys: false,
    };
  '';
in
{
  # Plaintext admin password (what the human types), generated once and synced
  # to Bitwarden. Node-RED never sees this directly; it checks the bcrypt hash
  # rendered from it by node-red-admin-env below.
  age.secrets.node-red-admin-password = {
    rekeyFile = ./secrets/node_red_admin_password.age;
    generator.script = "passphrase";
    bitwarden = {
      name = "Node-RED Admin Password";
      username = "admin";
      uris = { uri = "https://nodered.h.b.nel.family"; matchType = "host"; };
    };
  };

  # Key used to encrypt flow credentials on disk. Pinning it keeps saved
  # credentials readable across container recreations.
  age.secrets.node-red-credential-secret = {
    rekeyFile = ./secrets/node_red_credential_secret.age;
    generator.script = { pkgs, ... }: "${pkgs.openssl}/bin/openssl rand -hex 32";
  };

  # Render the runtime env the container reads: bcrypt the plaintext password
  # (Node-RED's bcryptjs accepts mkpasswd's $2b$ hashes) and pass through the
  # credentialSecret. Ordered before the container so the file always exists.
  #
  # This used to set RemainAfterExit = true, which quietly made the env file a
  # one-shot artefact of boot: the unit stayed "active" forever, so `Requires=`
  # from the container was always already satisfied and this never ran again.
  # Anything that cleared /run/node-red while the system was up therefore broke
  # node-red permanently, with nothing to regenerate the file.
  #
  # That is not hypothetical -- adding two unrelated systemd.tmpfiles.rules
  # elsewhere in the config restarted systemd-tmpfiles-resetup, /run/node-red
  # went with it, and the container then failed every start with
  #   Error: parsing file "/run/node-red/admin.env": no such file or directory
  # while this unit still reported active (exited) from three days earlier.
  #
  # So: no RemainAfterExit, and the unit goes inactive after each run. The
  # container's Requires= then genuinely re-runs it on every start, re-rendering
  # the env from the secrets. RuntimeDirectoryPreserve stops systemd deleting
  # the directory the moment the oneshot exits, which without RemainAfterExit it
  # otherwise would. The bcrypt salt differs each run; that is fine, Node-RED
  # only ever verifies the password against the current hash.
  systemd.services.node-red-admin-env = {
    description = "Render Node-RED admin env (bcrypt hash + credentialSecret)";
    before = [ "podman-node-red.service" ];
    requiredBy = [ "podman-node-red.service" ];
    path = [ pkgs.mkpasswd ];
    serviceConfig = {
      Type = "oneshot";
      RuntimeDirectory = "node-red";
      RuntimeDirectoryMode = "0700";
      RuntimeDirectoryPreserve = "yes";
    };
    script = ''
      pw="$(cat ${config.age.secrets.node-red-admin-password.path})"
      cs="$(cat ${config.age.secrets.node-red-credential-secret.path})"
      hash="$(mkpasswd -m bcrypt -R 8 "$pw")"
      umask 077
      cat > /run/node-red/admin.env <<EOF
      NODE_RED_ADMIN_USER=admin
      NODE_RED_ADMIN_PASSWORD_HASH=$hash
      NODE_RED_CREDENTIAL_SECRET=$cs
      EOF
    '';
  };

  virtualisation.oci-containers.containers.node-red = {
    image = "docker.io/nodered/node-red:latest-debian";
    environment = {
      "TZ" = "America/Denver";
      "NODE_RED_ENABLE_PROJECTS" = "true";
    };
    environmentFiles = [ "/run/node-red/admin.env" ];
    volumes = [
      "${dataDirs.level5}/node-red/data:/data"
      "${settingsJs}:/data/settings.js:ro"
    ];
    ports = [ "127.0.0.1:1880:1880" ];
    labels = {
      "io.containers.autoupdate" = "registry";
    };
  };
  services.nginx = {
    enable = true;
    virtualHosts = {
      "nodered.h.b.nel.family" = {
        forceSSL = true;
        enableACME = true;
        acmeRoot = null;
        extraConfig = ''
          client_max_body_size 0;
        '';
        locations = {
          "/" = {
            proxyPass = "http://localhost:1880";
            extraConfig = ''
              proxy_set_header Host $host;
              proxy_set_header Upgrade $http_upgrade;
              proxy_set_header Connection "upgrade";
            '';
          };
        };
      };
    };
  };
}
