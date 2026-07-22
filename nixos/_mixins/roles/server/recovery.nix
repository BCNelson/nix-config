# Server boot resilience.
#
# 1. Automatic rollback — systemd-boot Automatic Boot Assessment. Every new
#    boot entry is written with 3 tries; a boot only counts as "good" once it
#    reaches boot-complete.target (systemd-bless-boot then clears the counter).
#    After 3 boots that never get there (kernel panic, initrd failure, a wedged
#    mount dropping to emergency, ...) systemd-boot marks the entry bad and
#    boots the previous known-good generation instead — which already runs
#    sshd via the server role. This is the common "bad deploy" safety net.
#
# 2. A manually-selectable "recovery" specialisation — a stripped variant of
#    this same generation that only needs to bring SSH + networking up. It skips
#    data-pool imports and containers and never stops at a failed mount, so it
#    boots even when the datastore itself is what's wedging the machine. Choose
#    it from the boot menu at the console, or from a still-reachable system with
#    `bootctl set-oneshot <recovery-entry> && reboot`. Its sort key pins it to
#    the bottom of the menu so the automatic fallback in (1) always prefers a
#    full previous generation first — recovery is a deliberate last resort.
#
# systemd-boot only. The bootCounting guard means grub hosts (whiskey) simply
# skip (1); they still get the (2) recovery entry as a manual grub submenu.
{ config, lib, ... }:
{
  boot.loader.systemd-boot.bootCounting = lib.mkIf config.boot.loader.systemd-boot.enable {
    enable = true;
    tries = 3;
  };

  specialisation.recovery.configuration = {
    # Keep this entry at the bottom of the boot menu so automatic boot-counting
    # fallback selects a full previous generation before it. Overriding the
    # sort key uncouples the specialisation from its parent generation.
    boot.loader.systemd-boot.sortKey = lib.mkForce "zzz-recovery";

    # The whole point of the entry: guaranteed remote access.
    services.openssh.enable = lib.mkForce true;

    # Boot to a shell + SSH even when the datastore is the problem: don't import
    # the data pools, don't start containers, and don't stop at a failed mount.
    boot.zfs.extraPools = lib.mkForce [ ];
    virtualisation.docker.enable = lib.mkForce false;
    systemd.enableEmergencyMode = false;
  };
}
