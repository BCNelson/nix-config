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
  systemd.services.node-red-admin-env = {
    description = "Render Node-RED admin env (bcrypt hash + credentialSecret)";
    before = [ "podman-node-red.service" ];
    requiredBy = [ "podman-node-red.service" ];
    path = [ pkgs.mkpasswd ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
      RuntimeDirectory = "node-red";
      RuntimeDirectoryMode = "0700";
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
