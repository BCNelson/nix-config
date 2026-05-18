_:
{
  config = {
    system-manager.allowAnyDistro = true;

    # bcnelson already exists on the host with matching UID/GID/groups, so
    # userborn would only churn /etc/passwd for no benefit and races with the
    # home-manager activation service that depends on a stable User= lookup.
    services.userborn.enable = false;
    nix.enable = true;

    users.groups.bcnelson.gid = 1000;
    users.users.bcnelson = {
      isNormalUser = true;
      uid = 1000;
      group = "bcnelson";
      home = "/home/bcnelson";
      createHome = false;
    };

    home-manager.backupFileExtension = "bak";

    # Override plasma-workspace's /etc/pam.d/kde-fingerprint to bump the
    # lock-screen fingerprint timeout from 600s to 2h. Copied (not symlinked)
    # so PAM reads a real file; SELinux relabel is handled generically by
    # system-manager/_mixins/selinux.nix.
    environment.etc."pam.d/kde-fingerprint" = {
      mode = "0644";
      replaceExisting = true;
      text = ''
        auth    required    pam_env.so
        auth    [success=done default=bad]  pam_fprintd.so timeout=7200 max-tries=10
        auth    required    pam_deny.so

        auth        include       postlogin

        account     required      pam_nologin.so
        account     include       fingerprint-auth

        password    include       fingerprint-auth

        session     required      pam_selinux.so close
        session     required      pam_loginuid.so
        session     required      pam_selinux.so open
        session     optional      pam_keyinit.so force revoke
        session     required      pam_namespace.so
        session     include       fingerprint-auth
        session     include       postlogin
      '';
    };
  };
}
