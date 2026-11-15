{pkgs, ...}:
{
  
  imports = [
    ./appimage.nix
  ];

  services= {
    pcscd.enable = true;
    kmscon.enable = true;
  };
  programs.partition-manager.enable = true;

  # zen 7.3.4 panics at boot: its sched-alt merge left a second, unguarded
  # `worker->current_at = worker->task->...` line after the `if (worker->task)`
  # in process_one_work(). BH workqueue workers have no task, so the first BH
  # work item (xhci_pci registering the USB 3 root hub on sierra) dereferences
  # NULL+0x138 in ksoftirqd. Mainline has only the guarded line, so ride the
  # cached vanilla kernel (patching zen would mean building it locally).
  # Fixed in 7.3.4-zen2 (zen-kernel#425, nixpkgs#586820); this flips back to zen
  # by itself once nixos-unstable ships it. Drop the conditional after that.
  boot.kernelPackages =
    if pkgs.linuxPackages_zen.kernel.modDirVersion == "7.3.4-zen1"
    then pkgs.linuxPackages_latest
    else pkgs.linuxPackages_zen;
}
