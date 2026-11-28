{
  config,
  lib,
  pkgs,
  ...
}:
# OpenBao -- secrets for Forgejo Actions jobs, at bao.romeo.b.nel.family.
#
# Who can reach it: the tailnet only. *.romeo.b.nel.family is a wildcard CNAME
# to romeo's Tailscale address (main.tf), so no DNS change is needed, and nginx
# refuses anything outside Tailscale's ranges. Forgejo's runner is on romeo
# itself: rootless Podman job containers resolve the name to romeo's tailnet
# address and pasta turns the connection into local traffic from that address,
# which the allow list admits (checked from a job container, v4 and v6,
# 2026-11-27). The LAN name would not work from a job: pasta gives the
# container romeo's LAN IP, so 192.168.3.7 is the container itself.
#
# How jobs log in: JWT auth against Forgejo's Actions OIDC issuer
# (https://git.bcnelson.dev/api/actions). Nothing long-lived is stored in
# Forgejo. A job with `enable-openid-connect: true` asks Forgejo for an ID
# token with audience https://bao.romeo.b.nel.family and trades it for a
# 15-minute OpenBao token that can read ci/<owner>/<repo>/* and nothing else.
# Forgejo issues no ID token to fork pull requests (pull_request_target
# aside), so a fork cannot read a repo's secrets. Roles, policies and mounts
# are set up by `openbao-bootstrap` (./openbao/bootstrap.sh); see
# docs/openbao.md for using it from a workflow.
#
# Sealing: the static seal. Romeo reboots itself after auto-updates, and a
# Shamir-sealed server would stay sealed (and CI broken) until someone typed
# unseal keys in. The static key is an agenix secret, so the server unseals
# itself on start. What it still buys: the raft data on the vault, in ZFS
# snapshots, on vor and in borg is ciphertext without the key, and the key
# only exists in git encrypted to the master keys and in /run/agenix on romeo.
# It does not protect against root on romeo, which nothing here could.
let
  dataDirs = config.data.dirs;

  host = "bao.romeo.b.nel.family";
  port = 8200;

  # On the vault: level1 gets sanoid snapshots, syncoid to vor and borg
  # offsite (../backups.nix). /var/lib on romeo's root disk is backed up by
  # nothing. Raft's bolt DB is crash-safe, so an atomic ZFS snapshot of the
  # live directory is a usable backup.
  dataDir = "${dataDirs.level1}/openbao";

  # Bump the id whenever the key changes (rotation keeps the old one as
  # previous_key; see the static seal docs).
  unsealKeyId = "romeo-20261127";
  unsealCredential = "unseal-key";
in
{
  # 32-byte AES key, hex encoded. OpenBao reads the file *without* trimming
  # whitespace and picks the encoding by length (64 = hex), so the file must
  # be exactly 64 characters: no trailing newline.
  age.secrets.openbao_unseal_key = {
    rekeyFile = ./secrets/openbao_unseal_key.age;
    generator.script = { pkgs, ... }: ''printf '%s' "$(${pkgs.openssl}/bin/openssl rand -hex 32)"'';
  };

  services.openbao = {
    enable = true;
    settings = {
      ui = true;
      api_addr = "https://${host}";
      cluster_addr = "http://127.0.0.1:8201";

      # nginx terminates TLS and is the only client of this listener.
      listener.tcp = {
        type = "tcp";
        address = "127.0.0.1:${toString port}";
        tls_disable = true;
        # Record the real client in the audit log, not nginx.
        x_forwarded_for_authorized_addrs = "127.0.0.1/32";
        x_forwarded_for_reject_not_present = false;
      };

      storage.raft = {
        path = dataDir;
        node_id = "romeo";
      };
      # No disable_mlock: OpenBao 2.7 dropped mlock and warns about the
      # setting. The module's MemorySwapMax=0 keeps secrets out of swap instead.

      # Every request, HMAC'd, into romeo's journal. OpenBao 2.7 refuses to
      # enable audit devices over the API, so this is the only place to do it.
      audit.file.stdout = {
        description = "audit log to the journal";
        options.file_path = "stdout";
      };

      seal.static = {
        current_key_id = unsealKeyId;
        current_key = "file:///run/credentials/openbao.service/${unsealCredential}";
      };
    };
  };

  # A fixed user instead of the module's DynamicUser: the data lives on the
  # vault, outside StateDirectory, and has to keep its owner across restarts.
  users.users.openbao = {
    isSystemUser = true;
    group = "openbao";
  };
  users.groups.openbao = { };

  systemd.tmpfiles.rules = [ "d ${dataDir} 0700 openbao openbao - -" ];

  systemd.services.openbao = {
    # The module never restarts OpenBao on switch because a restart reseals it
    # under Shamir. With the static seal it unseals itself, so a restart costs
    # a second of downtime and config changes actually apply.
    restartIfChanged = lib.mkForce true;

    # level1 is a ZFS-native mount, which RequiresMountsFor alone does not
    # order against at boot; without zfs-mount.service OpenBao could start
    # first and initialise an empty raft store on the root disk.
    requires = [ "zfs-mount.service" ];
    after = [ "zfs-mount.service" ];
    unitConfig.RequiresMountsFor = [ dataDir ];

    serviceConfig = {
      DynamicUser = lib.mkForce false;
      # DynamicUser implied these; keep them now that it is off.
      ProtectSystem = "strict";
      PrivateTmp = true;
      RemoveIPC = true;
      NoNewPrivileges = true;
      ReadWritePaths = [ dataDir ];
      # systemd reads the agenix file as root and hands the service a private
      # copy, so the secret stays root-owned.
      LoadCredential = [ "${unsealCredential}:${config.age.secrets.openbao_unseal_key.path}" ];
    };
  };

  services.nginx.virtualHosts.${host} = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    extraConfig = ''
      allow 100.64.0.0/10;
      allow fd7a:115c:a1e0::/48;
      deny all;
    '';
    locations."/".proxyPass = "http://127.0.0.1:${toString port}";
  };

  environment.variables.BAO_ADDR = "https://${host}";

  environment.systemPackages = [
    (pkgs.writeShellApplication {
      name = "openbao-bootstrap";
      runtimeInputs = [
        config.services.openbao.package
        pkgs.jq
      ];
      text = builtins.readFile ./openbao/bootstrap.sh;
    })
  ];
}
