{ lib, ... }:
{
  imports = [
    ../docker.nix
  ];

  services.openssh = {
    enable = true;
    openFirewall = true;
  };
  # Add nix-ld so that we can use vscode remote ssh
  programs.nix-ld.enable = true;

  # Passwordless sudo for wheel-group users on servers.
  security.sudo.wheelNeedsPassword = false;

  # fwupd (enabled globally in common.nix) runs an hourly `fwupdmgr refresh`
  # via fwupd-refresh.service as a sessionless DynamicUser. On a headless
  # server there is no polkit agent, so the metadata refresh fails with
  # "Failed to obtain auth". Drop the timer so it never runs; fwupd stays
  # available for manual `fwupdmgr` use.
  systemd.timers.fwupd-refresh.wantedBy = lib.mkForce [ ];
}
