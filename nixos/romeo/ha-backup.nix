{ config, lib, ... }:
let
  # Home Assistant (192.168.3.6, HA OS) pushes backups here with its
  # "SFTP Storage" backup agent. Configure the agent with:
  #   host 192.168.3.7, port 22, user ha-backup, backup location /backups
  # and upload the private key from Bitwarden ("Home Assistant backup SFTP key").
  user = "ha-backup";

  # The keypair is an agenix-rekey generated secret. The generator drops the
  # public half next to the .age file (same trick as nix_cache_key), and it is
  # read at evaluation time, so it has to be committed in the clear. Guarded so
  # the config still evaluates before `just new-secrets` has run.
  keyFile = ../../secrets/store/romeo/ha_backup_ssh_key.age;
  pubKeyFile = ../../secrets/store/romeo/ha_backup_ssh_key.pub;
  pubKey = lib.optionalString (builtins.pathExists pubKeyFile)
    (lib.removeSuffix "\n" (builtins.readFile pubKeyFile));
  haAddress = "192.168.3.6";

  # sshd refuses to chroot unless every path component is root-owned and not
  # group/world-writable. /mnt/vault/** is 0777, so chroot into a root-owned
  # stub under /srv and bind the real (snapshotted, borg'd, syncoid'd) level2
  # directory into it.
  chroot = "/srv/ha-backup";
  storage = "${config.data.dirs.level2}/homeassistant-backups";

  # Only the SFTP requests the sftp_storage integration issues: chdir
  # (realpath/stat), listdir, open/read/write for upload+download, remove for
  # retention. No mkdir/rename/symlink/setstat etc.
  allowedRequests = "open,close,read,write,lstat,stat,fstat,opendir,readdir,remove,realpath,limits";
in
{
  users.groups.${user} = { };
  users.users.${user} = {
    isSystemUser = true;
    group = user;
    home = "/var/empty";
    createHome = false;
    # Key-level lockdown on top of the Match block: source address pinned to
    # the HA box, no forwarding/pty/agent/X11.
    openssh.authorizedKeys.keys = lib.optional (pubKey != "")
      ''restrict,from="${haAddress}" ${pubKey}'';
  };

  # Only HA needs the private key, so it is `intermediary`: kept (and
  # rekeyable) in the repo but never decrypted onto romeo. It is declared here
  # so `just new-secrets` generates it and `just sync-secrets` files it in
  # Bitwarden, which is where it gets copied into HA from.
  age.secrets.ha_backup_ssh_key = {
    rekeyFile = keyFile;
    intermediary = true;
    generator.script = { pkgs, lib, file, ... }: ''
      tmp=$(mktemp -d)
      trap 'rm -rf "$tmp"' EXIT
      ${pkgs.openssh}/bin/ssh-keygen -q -t ed25519 -N "" \
        -C "homeassistant-backup@${haAddress}" -f "$tmp/key"
      cp "$tmp/key.pub" ${lib.escapeShellArg (lib.removeSuffix ".age" file + ".pub")}
      cat "$tmp/key"
    '';
    # A secure note: the decrypted private key becomes the note body.
    bitwarden = {
      name = "Home Assistant backup SFTP key";
      fields = [
        { name = "host"; value = "192.168.3.7"; }
        { name = "port"; value = "22"; }
        { name = "username"; value = user; }
        { name = "backup location"; value = "/backups"; }
        { name = "config"; value = "nixos/romeo/ha-backup.nix"; }
      ];
    };
  };

  systemd.tmpfiles.rules = [
    "d ${chroot}         0755 root    root    - -"
    "d ${chroot}/backups 0755 root    root    - -"
    "d ${storage}        0700 ${user} ${user} - -"
  ];

  fileSystems."${chroot}/backups" = {
    device = storage;
    fsType = "none";
    options = [
      "bind"
      "nofail"
      "x-systemd.requires-mounts-for=${config.data.dirs.level2}"
    ];
  };

  services.openssh.extraConfig = ''
    Match User ${user}
      ChrootDirectory ${chroot}
      ForceCommand internal-sftp -d /backups -u 0077 -p ${allowedRequests}
      AuthenticationMethods publickey
      PasswordAuthentication no
      KbdInteractiveAuthentication no
      AllowTcpForwarding no
      AllowStreamLocalForwarding no
      AllowAgentForwarding no
      PermitTunnel no
      PermitTTY no
      X11Forwarding no
  '';
}
