{ ... }: {
  imports = [
    ./magicmirror.nix
  ];

  systemd.timers.podman-auto-update = {
    enable = true;
  };
}