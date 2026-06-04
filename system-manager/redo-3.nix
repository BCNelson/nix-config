_:
{
  config = {
    system-manager.allowAnyDistro = true;

    # bcnelson already exists on the host with matching UID/GID/groups, so
    # userborn would only churn /etc/passwd for no benefit and races with the
    # home-manager activation service that depends on a stable User= lookup.
    services.userborn.enable = false;

    users.groups.bcnelson.gid = 1000;
    users.users.bcnelson = {
      isNormalUser = true;
      uid = 1000;
      group = "bcnelson";
      home = "/home/bcnelson";
      createHome = false;
    };

    home-manager.backupFileExtension = "bak";

    # Disable USB autosuspend on the Goodix 27c6:609c fingerprint reader so
    # it stays responsive while the lock screen is up.
    environment.etc."udev/rules.d/60-goodix-fingerprint-no-suspend.rules" = {
      mode = "0644";
      replaceExisting = true;
      text = ''
        # Goodix 27c6:609c — keep fingerprint reader awake so unlock prompt stays responsive
        ACTION=="add", SUBSYSTEM=="usb", ATTR{idVendor}=="27c6", ATTR{idProduct}=="609c", TEST=="power/control", ATTR{power/control}="on"
      '';
    };

    # Strip Fedora's `--no-timeout` from fprintd. The Goodix reader's libusb
    # state gets stuck after a system suspend/resume; if fprintd stays
    # running, every later kscreenlocker auth silently fails on a wedged
    # Claim. Restoring the default idle-shutdown makes fprintd exit ~30s
    # after the last DBus call, so the next pam_fprintd activates a fresh
    # daemon process with a fresh USB context.
    environment.etc."systemd/system/fprintd.service.d/override.conf" = {
      mode = "0644";
      replaceExisting = true;
      text = ''
        [Service]
        ExecStart=
        ExecStart=/usr/libexec/fprintd
      '';
    };

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
