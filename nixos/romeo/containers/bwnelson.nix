# A sandboxed NixOS box for bwnelson (brother): coding, learning Linux, and
# running cyclus fuel-cycle simulations.
#
# Threat model is incompetence, not malice -- he is trusted, but a mistake of
# his must not be able to take down the family's photos, media or chat. That
# choice is what makes a systemd-nspawn container the right tool rather than a
# microvm: the shared kernel only matters against someone actively attacking
# it, and cgroup limits handle the accident case well. Everything below is
# either "give him a real box" or "bound the damage of a mistake".
_:
let
  # romeo's LAN interface, used as the NAT external interface so the container
  # can reach the internet -- tailscale's coordination server, plus the
  # conda-forge and nixpkgs traffic he will generate constantly. romeo uses
  # NetworkManager with DHCP, so this was read off the host
  # (`ip -br -4 addr | grep 192.168.3.7`) rather than set anywhere in the repo.
  lanInterface = "enp12s0";

  # Deliberately inside 10.0.0.0/8: romeo's unbound already carries
  # "10.0.0.0/8 allow" in its access-control list (../unbound.nix), so the
  # container can use romeo as its resolver without touching unbound at all.
  hostAddress = "10.233.1.1";
  localAddress = "10.233.1.2";
in
{
  containers.bwnelson = {
    autoStart = true;

    # Its own network namespace. Not optional here: a second tailscaled
    # sharing romeo's netns would fight romeo's own over the same interface
    # and state directory.
    privateNetwork = true;
    inherit hostAddress localAddress;

    # UID/GID namespacing -- root inside the container maps to an
    # unprivileged UID on romeo. "pick" auto-selects a non-overlapping range
    # and is what the option's own documentation recommends. This is the
    # difference between "he has root in a container" and "he has root on
    # romeo", and it is off by default, so it is easy to leave unset.
    privateUsers = "pick";

    # tailscaled needs /dev/net/tun and CAP_NET_ADMIN.
    enableTun = true;

    # romeo rebuilds hourly under services.bcnelson.autoUpdate. Without this,
    # any unrelated change to romeo's closure would restart the container and
    # kill whatever he was running -- for a multi-hour cyclus simulation that
    # means silently losing the run. Reboots still restart it via autoStart,
    # but at least a plain switch won't.
    restartIfChanged = false;

    config = { pkgs, lib, ... }: {
      # A brand-new container, so track the current release rather than
      # inheriting the 23.05 the hosts in this repo carry.
      system.stateVersion = lib.trivial.release;

      users.users.bwnelson = {
        isNormalUser = true;
        description = "Brother";
        extraGroups = [ "wheel" ];
        # Empty on purpose: Tailscale SSH on his own tailnet authenticates
        # him, so he can get in before ever sending a key. Add it here when
        # he does.
        openssh.authorizedKeys.keys = [ ];
      };

      # He is root in his own sandbox by design, and has no password set --
      # Tailscale SSH is the way in. Prompting for a password he does not
      # have would only lock him out of sudo.
      security.sudo.wheelNeedsPassword = false;

      services.openssh = {
        enable = true;
        settings = {
          PasswordAuthentication = false;
          PermitRootLogin = "no";
        };
      };

      # Joined to his own tailnet, not ours: `tailscale up --ssh` from a root
      # login, with the login URL handed to him. Our ACLs never see this node.
      services.tailscale = {
        enable = true;
        useRoutingFeatures = "client";
        openFirewall = true;
      };

      networking = {
        hostName = "bwnelson";
        # Do not inherit romeo's /etc/resolv.conf. NetworkManager runs there
        # with dns=none and resolved is disabled, so whatever it points at
        # means something different inside this namespace. Name romeo's
        # unbound explicitly instead.
        useHostResolvConf = false;
        nameservers = [ "192.168.3.7" ];
        firewall = {
          enable = true;
          allowedTCPPorts = [ 22 ];
          trustedInterfaces = [ "tailscale0" ];
        };
      };

      # What makes conda-forge binaries run here. cyclus is not in nixpkgs,
      # and `micromamba install -c conda-forge cyclus cycamore` is the install
      # path upstream documents -- those binaries expect a conventional
      # dynamic loader at /lib64, which nix-ld provides. If a conda package
      # still fails to link, add the missing library to programs.nix-ld.libraries.
      programs.nix-ld.enable = true;

      nix.settings.experimental-features = [ "nix-command" "flakes" ];

      time.timeZone = "America/Los_Angeles";

      environment.systemPackages = with pkgs; [
        # cyclus lives here, via conda-forge. See docs/guest-container.md.
        micromamba

        # Shell basics, and the tools worth having while learning Linux.
        # tmux and mosh especially: romeo reboots on kernel updates, so
        # anything he runs in a bare SSH session gets cut.
        tmux
        mosh
        git
        vim
        neovim
        htop
        btop
        tree
        file
        ripgrep
        fd
        jq
        curl
        wget
        rsync
        unzip
        man-pages
        man-pages-posix

        # Enough of a toolchain to build things without reaching for nix-shell.
        gcc
        gnumake
        cmake
        pkg-config
        python3
        uv
      ];
    };
  };

  # Masquerade the container's outbound traffic. romeo has no NAT today, and
  # the container needs it for conda/nixpkgs downloads regardless of how he
  # reaches the box. docker already sets net.ipv4.ip_forward=1, and this repo
  # leaves networking.nftables off, so these iptables rules coexist with
  # docker's own.
  networking.nat = {
    enable = true;
    internalInterfaces = [ "ve-bwnelson" ];
    externalInterface = lanInterface;
  };

  # The part that actually protects romeo.
  #
  # romeo has no swap at all -- 2.hardware-configuration.nix sets
  # swapDevices = [ ] and there is no zramSwap either, unlike whiskey. So on
  # memory exhaustion the kernel OOM killer picks its victim by badness score
  # across the whole box, and the fattest processes on romeo are immich's
  # postgres and jellyfin. MemoryMax is what confines that kill to this
  # cgroup: it is not about rationing him, it is about making his mistake his
  # problem. Cyclus simulations can balloon unintentionally, which is exactly
  # the case this guards.
  #
  # MemoryHigh sits below MemoryMax so he gets throttled and reclaimed first
  # and his process slows down instead of dying outright. 24G of 96G is a
  # quarter of the box; ZFS ARC will happily take up to half but is
  # reclaimable under pressure, so this is a ceiling rather than a
  # reservation. Dial it down if ARC pressure shows up in monitoring.
  #
  # CPUWeight rather than CPUQuota is deliberate. romeo does jellyfin
  # transcoding and frigate object detection, both latency-sensitive. A hard
  # quota would waste idle cores at 3am when a long simulation could have the
  # whole machine; a low weight lets him use everything when it is quiet and
  # yield the instant something at the default weight of 100 wants CPU. Even
  # a runaway infinite loop stays polite under this.
  #
  # IOWeight for the same reason -- cyclus writes large sqlite/HDF5 output
  # and ZFS contention would show up as jellyfin stuttering. TasksMax is the
  # fork-bomb backstop.
  # Keep him off the rest of the LAN. He is root in the container and it sits
  # on his tailnet, not ours, so `tailscale up --advertise-routes=
  # 192.168.3.0/24` (or an exit node) would hand our whole LAN to his tailnet
  # and anyone he shares it with -- nothing on our side would ever see it.
  # This lives on romeo because ve-bwnelson is a host interface. Traffic to
  # romeo's own 192.168.3.7 goes through INPUT rather than FORWARD, so unbound
  # and romeo's opened ports stay reachable.
  networking.firewall.extraCommands = ''
    iptables -I FORWARD -i ve-bwnelson -d 192.168.3.0/24 -j DROP
  '';
  networking.firewall.extraStopCommands = ''
    iptables -D FORWARD -i ve-bwnelson -d 192.168.3.0/24 -j DROP || true
  '';

  systemd.services."container@bwnelson".serviceConfig = {
    MemoryHigh = "20G";
    MemoryMax = "24G";
    CPUWeight = 50;
    IOWeight = 50;
    TasksMax = 4096;
  };
}
