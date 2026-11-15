{pkgs, inputs, ...}:
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
  # NULL+0x138 in ksoftirqd. Fixed in 7.3.4-zen2 (zen-kernel#425,
  # nixpkgs#586820), which reached nixos-unstable-small (and its binary cache)
  # days before nixos-unstable, so take the kernel from there while our channel
  # still has zen1 -- patching zen1 instead would mean building it locally.
  # Once nixos-unstable ships zen2 this falls back to the channel's own kernel;
  # drop the conditional then.
  boot.kernelPackages =
    if pkgs.linuxPackages_zen.kernel.modDirVersion == "7.3.4-zen1"
    then inputs.nixpkgs-unstable-small.legacyPackages.${pkgs.stdenv.hostPlatform.system}.linuxPackages_zen
    else pkgs.linuxPackages_zen;
}
