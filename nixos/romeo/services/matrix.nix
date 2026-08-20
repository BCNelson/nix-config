{
  config,
  lib,
  pkgs,
  ...
}: let
  dataDirs = config.data.dirs;

  # The RocksDB directory. On the vault rather than romeo's root pool, so it
  # rides the pool's redundancy and sanoid's snapshots (72 hourly / 31 daily /
  # 24 weekly / 12 monthly / 2 yearly on vault/data/level4).
  #
  # level4 "Medium" is the honest tier for it. What lives here is E2EE device
  # identities, room keys and message history -- annoying to lose, but not
  # irreplaceable: the *accounts* are declared in this file and recreated on
  # the next boot, and OpenClaw logs back in from its agenix password. Note
  # level4 has no borg job (it is commented out in ../backups.nix) and no
  # syncoid replication, so this is protected against a disk failing and
  # against "I deleted something an hour ago", not against losing the pool.
  #
  # ../nixarr.nix puts its state on level4 the same way.
  databaseDir = "${dataDirs.level4}/continuwuity";

  # Permanent. server_name is the suffix on every user id, room id and event
  # this server ever creates, and it is baked into the database on first boot;
  # changing it later means throwing the database away and starting over.
  #
  # It is also the federation identity, which is why this is a public name
  # rather than something under h.b: remote servers have to resolve it and
  # reach it. See the vhost at the bottom for how that works without opening
  # port 8448.
  serverName = "bot.nel.family";

  # Loopback only -- nginx below owns TLS, the certificate and the public
  # surface. The module's default is 6167.
  port = 6167;

  # Uploads. The two numbers have to agree: nginx rejects first with 413 if it
  # is the smaller one, continuwuity rejects second with M_TOO_LARGE.
  maxRequestSizeMb = 50;

  # THE DECLARATIVE ACCOUNT ROSTER. Adding an agent is one entry here plus a
  # rebuild: agenix generates its password, continuwuity creates the account on
  # next start, and the password is available to whatever consumes it.
  #
  # A list rather than an attrset because ORDER IS SIGNIFICANT: continuwuity
  # invites the *first* user ever created to the admin room ("@x has been
  # invited to the admin room as the first user"), and an attrset would decide
  # that by alphabetical accident. bcnelson stays first.
  #
  # Names must be plain [a-z0-9_] -- they are interpolated into an env var name
  # below, and into an admin command line that is split on whitespace.
  accounts = [
    {
      name = "bcnelson";
      description = "My own account. First in the list, so this is the one continuwuity invites to the admin room; every `!admin ...` command is typed from here.";
    }
    {
      name = "openclaw";
      description = "The OpenClaw gateway on this host. ./openclaw.nix reads the same password out of agenix and logs in with it, so there is no token to mint or paste.";
    }
  ];

  # PW_BCNELSON, PW_OPENCLAW, ... -- the envsubst placeholders that
  # age-template replaces with the decrypted passwords at activation.
  pwVar = account: "PW_${lib.toUpper account.name}";
  # "$PW_BCNELSON" as a literal for the template body. Built by concatenation
  # rather than written inline: in a Nix string "$${x}" is the escape for a
  # literal "${x}" and would emit the placeholder name unexpanded.
  pwRef = account: "$" + pwVar account;

  secretName = account: "matrix-password-${account.name}";
in {
  # One generated password per account, and deliberately `alnum` rather than
  # the `passphrase` generator ./tendant.nix uses: this value is substituted
  # into an admin command line that continuwuity splits on whitespace, so a
  # six-word xkcd passphrase would be parsed as six arguments and the account
  # would end up with the first word as its password.
  #
  # Each is a *login* item in Bitwarden because these are typed by a human --
  # mine into Element on every new client, and (in a recovery scenario) the
  # gateway's into whatever is being debugged. The URI match makes them
  # autofill on the homeserver's own login page.
  age.secrets = lib.listToAttrs (map (account:
    lib.nameValuePair (secretName account) {
      rekeyFile = ./secrets/matrix_password_${account.name}.age;
      generator.script = "alnum";
      bitwarden = {
        name = "Matrix @${account.name}:${serverName}";
        username = "@${account.name}:${serverName}";
        uris = {
          uri = "https://${serverName}";
          matchType = "host";
        };
        notes = "${account.description}\n\nProvisioned declaratively by nixos/romeo/services/matrix.nix -- the account is created from this password on the first boot after it is added, and the value here is the source of truth. Changing it in Bitwarden changes nothing; rotate with `agenix edit` + `just rekey` + rebuild, then `!admin users reset-password` from the admin room, because admin_execute only ever *creates* accounts.";
      };
    })
  accounts);

  # This file is why the passwords are safe to declare at all.
  #
  # continuwuity's own config is a TOML file in the world-readable Nix store
  # (the module generates it with pkgs.formats.toml), and `extraEnvironment`
  # lands in the unit file, which is also in the store. Neither can hold a
  # password. But continuwuity reads every config key from the environment as
  # well, via figment's CONTINUWUITY_ prefix -- including array-valued keys,
  # which figment parses out of the env value as TOML. So the one key that
  # needs secrets is delivered here instead, from /run, mode 0400.
  #
  # Verified against matrix-continuwuity 26.7.2 by hand before writing this:
  # CONTINUWUITY_ADMIN_EXECUTE='["users create-user testbot <pw>"]' created the
  # account at startup, and the account then accepted a normal
  # /_matrix/client/v3/login password login.
  #
  # Owned by root because systemd reads EnvironmentFile as root before it drops
  # to the (dynamic) service user.
  age-template.files.continuwuity-env = {
    vars = lib.listToAttrs (map (account:
      lib.nameValuePair (pwVar account) config.age.secrets.${secretName account}.path)
    accounts);
    owner = "root";
    group = "root";
    mode = "0400";
    content = ''
      CONTINUWUITY_ADMIN_EXECUTE=[${
        lib.concatMapStringsSep "," (account: "\"users create-user ${account.name} ${pwRef account}\"") accounts
      }]
    '';
  };

  services.matrix-continuwuity = {
    enable = true;
    settings.global = {
      server_name = serverName;

      # nginx is the only thing that talks to this.
      address = ["127.0.0.1"];
      port = [port];

      # Federation is on, so this server is reachable by strangers. Closed
      # registration is the single most important consequence: an open
      # registration server is spam infrastructure within days, and
      # continuwuity itself prints a warning banner in the admin room when it
      # detects one. Accounts come from admin_execute above instead.
      allow_registration = false;
      allow_federation = true;
      allow_encryption = true;

      # Makes the create-user commands idempotent. Without this, the *second*
      # boot fails: the command errors with "Username is not available." and
      # (by default) that is fatal, so the homeserver refuses to start because
      # the accounts it was asked to create already exist. With it, the failure
      # is logged and startup continues -- which is exactly the "declare it,
      # rebuild, it exists, rebuilding again is a no-op" behaviour wanted here.
      admin_execute_errors_ignore = true;

      # Serves /.well-known/matrix/{client,server} from continuwuity itself
      # rather than hand-written JSON in the vhost.
      #
      # `server` is what keeps port 8448 closed: without delegation a remote
      # server would try ${serverName}:8448, but this tells it to use 443
      # instead, where nginx already terminates TLS with a real certificate.
      # `client` is what lets a phone client find the homeserver by typing
      # "${serverName}" instead of a full URL.
      well_known = {
        client = "https://${serverName}";
        server = "${serverName}:443";
      };

      max_request_size = maxRequestSizeMb * 1000 * 1000;

      # Defaults to true, which makes the server GET
      # continuwuity.org/.well-known/continuwuity/announcements on a timer.
      # Harmless, but it is an outbound call this host does not need to make
      # and a signal it does not need to emit; upgrades come from nixpkgs.
      allow_announcements_check = false;

      # Only used to fetch other servers' signing keys. matrix.org is the
      # conventional notary; the option's own documentation notes continuwuity
      # cannot serve batched key requests, so this list should contain Synapse
      # servers rather than other conduwuit-family servers.
      trusted_servers = ["matrix.org"];
    };
  };

  # Public, unlike every other vhost on this host: federation means arbitrary
  # remote homeservers must reach /_matrix/federation, so the tailnet/LAN
  # allowlist used by ./openclaw.nix and ./goose.nix cannot be applied here.
  # Closed registration and per-account passwords are what bound the exposure
  # instead.
  #
  # Reaching it needs two records outside this file, both already added:
  #   - main.tf: `bot` CNAME -> h.b.nel.family, so the public name lands on the
  #     ingress, exactly like social.nel.family does for GoToSocial.
  #   - ../unbound.nix: bot.nel.family A 192.168.3.7 in the RPZ zone, so LAN
  #     clients hairpin straight to romeo instead of trying to reach the WAN
  #     address from inside the network.
  # Relocating the database takes three coordinated changes, because the module
  # pins `database_path` as a readOnly option (it is "/var/lib/continuwuity/"
  # and cannot be set) on the grounds that the unit uses StateDirectory. So the
  # path stays put and the storage underneath it moves.
  #
  # 1. DynamicUser has to go. With it on, StateDirectory=continuwuity puts the
  #    real directory at /var/lib/private/continuwuity and leaves
  #    /var/lib/continuwuity as a symlink to it -- which a bind mount at that
  #    path would collide with. Turning it off is free here: the module already
  #    declares a static continuwuity user and group unconditionally (it only
  #    skips them if `user`/`group` are overridden), so User=/Group= resolve to
  #    a real account and StateDirectory chowns the mounted directory to it.
  #
  # 2. RequiresMountsFor, so the unit cannot start before the bind mount is
  #    live. Without it a boot that races the vault would have systemd create
  #    and chown an empty /var/lib/continuwuity on the root pool and
  #    continuwuity would happily build a second, hidden database there.
  systemd.services.continuwuity = {
    serviceConfig = {
      DynamicUser = lib.mkForce false;
      # The module exposes only `extraEnvironment`, which renders into the unit
      # file in the world-readable store; the passwords have to arrive from
      # /run instead. Note the unit is `continuwuity`, not
      # `matrix-continuwuity`.
      EnvironmentFile = config.age-template.files.continuwuity-env.path;
    };
    unitConfig.RequiresMountsFor = "/var/lib/continuwuity";
  };

  # 3. The bind mount itself. This is deliberately a host-level mount rather
  #    than the unit's own BindPaths=: BindPaths is applied inside the service's
  #    mount namespace, which is set up *after* systemd has already created and
  #    chowned the StateDirectory in the host namespace -- so the ownership
  #    would land on the shadowed empty directory and the service would find an
  #    unwritable database directory.
  fileSystems."/var/lib/continuwuity" = {
    device = databaseDir;
    # "none" is how a bind mount declares "no filesystem of its own"; the
    # option is mandatory and there is no default.
    fsType = "none";
    options = ["bind"];
    depends = [dataDirs.level4];
  };

  # StateDirectoryMode is 0700 and systemd will chown the mount point on start,
  # but the directory has to exist on the vault before anything can bind it.
  systemd.tmpfiles.rules = [
    "d ${databaseDir} 0700 continuwuity continuwuity -"
  ];

  services.nginx.virtualHosts."${serverName}" = {
    forceSSL = true;
    enableACME = true;
    acmeRoot = null;
    http2 = true;
    extraConfig = "client_max_body_size ${toString maxRequestSizeMb}M;";
    locations."/" = {
      proxyPass = "http://127.0.0.1:${toString port}";
      # Client sync is a long poll (30s by default, and clients pick longer),
      # so the stock 60s read timeout would sever every idle connection.
      extraConfig = ''
        proxy_read_timeout 600s;
        proxy_send_timeout 600s;
      '';
    };
  };
}
