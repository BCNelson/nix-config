{ config, lib, pkgs, ... }:

# delta-* are Dell Wyse 3040 thin clients: Atom x5-Z8350, 2 GiB of RAM, 8 GiB
# of eMMC. They cannot build this flake and they cannot even evaluate it —
# a full evaluation of this repo peaks well above the RAM available here.
#
# So they don't. romeo builds delta's system closure (see
# services.bcnelson.closurePublisher in nixos/romeo/services/closurePublisher.nix)
# and publishes both the closure and a manifest naming its store path to
# nixcache.nel.family. This host downloads that closure and activates it.
# There is no /config checkout and no nixos-rebuild here; `services.bcnelson.autoUpdate`
# is deliberately *not* used.
#
# Everything below the update wiring exists to keep 2 GiB of RAM and 8 GiB of
# flash sufficient.

let
  cacheUrl = "https://nixcache.nel.family";
in
{
  imports = [
    ../_mixins/roles/tailscale.nix
    ./disks.nix
    ./kiosk.nix
  ];

  age.secrets.ntfy_refresh_topic.rekeyFile = ../../secrets/store/ntfy_autoUpdate_topic.age;

  services.bcnelson.remoteUpdate = {
    enable = true;
    inherit cacheUrl;
    manifestUrl = "${cacheUrl}/system/${config.networking.hostName}";
    reboot = true;

    # A push is what normally brings an update in. romeo needs time to pull,
    # rebuild itself and then build this host's closure before there is
    # anything new to fetch, so the check is deliberately delayed rather than
    # immediate. Polling is only the backstop for a push that was missed
    # (host was off, message dropped, publish ran long).
    refreshInterval = "6h";
    ntfy-refresh = {
      enable = true;
      topicFile = config.age.secrets.ntfy_refresh_topic.path;
      delay = "30m";
    };

    # Signature verification is off until romeo's cache key is pinned here.
    # romeo generates the key on its first publish run; read it with
    # `just cache-key`, paste it into trustedPublicKeys, and flip this to true.
    # Until then the closure is trusted on the strength of the HTTPS connection
    # to nixcache.nel.family alone — note this only relaxes *this* fetch, the
    # host's global require-sigs stays on.
    checkSignatures = false;
    trustedPublicKeys = [ ];

    # ntfy_topic is declared by the tailscale role.
    ntfy = {
      enable = true;
      topicFile = config.age.secrets.ntfy_topic.path;
    };

    # Enable once secrets/store/cadence/checks/remote-update-delta.age exists
    # (`just generate-secrets`):
    #   healthCheck = {
    #     enable = true;
    #     url = "https://health.b.nel.family";
    #     uuidFile = config.age.secrets.cadence_check_remote_update_delta.path;
    #   };
  };

  # --- Memory -------------------------------------------------------------
  #
  # 2 GiB, no swap partition (see disks.nix). zram gives back roughly the
  # difference: zstd hits ~3:1 on anonymous pages, so 150% of RAM as a zram
  # device costs ~500 MiB of real memory at full utilisation while holding
  # ~3 GiB of pages.
  zramSwap = {
    enable = true;
    algorithm = "zstd";
    memoryPercent = 150;
  };

  boot.kernel.sysctl = {
    # zram is RAM: paging to it is far cheaper than dropping page cache and
    # re-reading from slow eMMC, so bias hard towards swapping.
    "vm.swappiness" = 180;
    # Swap readahead is pointless when the "device" has no seek cost, and it
    # wastes both CPU and memory decompressing pages nobody asked for.
    "vm.page-cluster" = 0;
    "vm.watermark_boost_factor" = 0;
    "vm.watermark_scale_factor" = 125;
    # Keep more dentry/inode cache; re-populating it means eMMC reads.
    "vm.vfs_cache_pressure" = 50;
    # eMMC writeback is slow. Small dirty limits keep a big flush from
    # stalling the whole machine.
    "vm.dirty_ratio" = 10;
    "vm.dirty_background_ratio" = 5;
  };

  # A tmpfs /tmp would come straight out of the 2 GiB.
  boot.tmp = {
    useTmpfs = false;
    cleanOnBoot = true;
  };

  # Under real pressure, kill the offender promptly instead of letting the
  # machine livelock in reclaim — on a kiosk, a restarted browser beats an
  # unresponsive box. Defaults (10% free memory and swap) are about right
  # once zram is in the picture.
  services.earlyoom.enable = true;

  # The runtime journal lives in /run, i.e. in RAM. Cap it, and cap the
  # on-disk journal too since flash is the other scarce resource.
  services.journald.extraConfig = ''
    Compress=yes
    SystemMaxUse=64M
    SystemMaxFileSize=16M
    RuntimeMaxUse=16M
  '';

  # A core dump from the browser is a multi-hundred-MiB write to an 8 GiB
  # eMMC, taken at exactly the moment the machine is already out of memory.
  systemd.coredump.enable = false;

  # --- Storage ------------------------------------------------------------

  nix = {
    settings = {
      # This host must never build anything. max-jobs = 0 turns an accidental
      # local build into an immediate, legible failure instead of an OOM.
      max-jobs = 0;
      cores = 1;
      # Nothing here is a dev machine, and both of these pin build inputs in
      # the store forever. common.nix sets them for the workstations.
      keep-outputs = lib.mkForce false;
      keep-derivations = lib.mkForce false;
      # Automatically GC when the filesystem gets tight, not just on the timer.
      min-free = 512 * 1024 * 1024;
      max-free = 2048 * 1024 * 1024;
      # `nix copy` buffers downloads in memory; the default is generous for a
      # machine with 2 GiB.
      download-buffer-size = 32 * 1024 * 1024;
    };
    # common.nix sets this with mkDefault, so a plain override is enough.
    gc.options = "--delete-older-than 3d";
    optimise.automatic = true;
    # No channels: this host never evaluates anything from a channel.
    channel.enable = false;
  };

  # Three generations is already more rollback than an 8 GiB device can afford.
  boot.loader.systemd-boot.configurationLimit = 3;

  # The NixOS manual, man pages and info pages are a few hundred MiB of a
  # filesystem that does not have a few hundred MiB spare. Set individually
  # rather than via profiles/minimal.nix, which also flips noXlibs and would
  # force a from-source rebuild of the browser stack.
  documentation = {
    enable = false;
    doc.enable = false;
    info.enable = false;
    man.enable = false;
    nixos.enable = false;
  };
  programs.command-not-found.enable = false;

  # perl, rsync and strace ship in the system profile by default.
  environment.defaultPackages = [ ];

  # journald already rotates; logrotate is another timer and another closure.
  services.logrotate.enable = false;
  services.udisks2.enable = false;
  system.disableInstallerTools = true;

  # The default font set (noto-fonts and friends) is several hundred MiB. One
  # good general-purpose family plus an emoji fallback is enough for a kiosk.
  fonts = {
    enableDefaultPackages = false;
    packages = with pkgs; [
      dejavu_fonts
      noto-fonts-emoji
    ];
  };

  # linux-firmware is ~1 GiB unpacked. Nothing on this board needs it: the
  # Cherry Trail iGPU has no DMC firmware and the Realtek NIC works without
  # its optional blob. Turn this back on if the optional Wi-Fi card is fitted.
  hardware.enableRedistributableFirmware = false;

  # fwupd has no firmware to offer for this board and is a sizeable closure.
  # common.nix turns it on for the machines where it earns its place.
  services.fwupd.enable = lib.mkForce false;

  hardware.graphics = {
    enable = true;
    # Cherry Trail is older than the iHD driver's support window; i965 is what
    # gives this SoC hardware video decode, which matters when the CPU is an
    # Atom and the job is rendering a web page.
    extraPackages = [ pkgs.intel-vaapi-driver ];
  };
}
