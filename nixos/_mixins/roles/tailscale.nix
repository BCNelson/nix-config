{ config, pkgs, lib, thinClient ? false, ... }: {
  environment.systemPackages = with pkgs; [
    tailscale
  ] ++ lib.optional (!thinClient) jq; # only the autoconnect script parses status
  services.tailscale = {
    enable = true;
    package = pkgs.unstable.tailscale;
  };

  # The autoconnect script exists to push the auth URL to a phone, and the ntfy
  # topic it posts to is an agenix secret. A thin client must declare no secrets
  # at all -- that is what lets a freshly installed one evaluate in CI and
  # finish unattended, and this secret is the specific one that used to force a
  # rekey onto a machine incapable of performing one (see the bcnelson user
  # mixin, which drops its own copy for the same reason).
  #
  # So on a thin client tailscaled runs but nothing auto-authenticates. These
  # are installed by hand at a console or over LAN ssh anyway, and joining the
  # tailnet is a one-time `tailscale up` at that same moment. The auth URL goes
  # to the terminal you are already sitting in front of, which is where the
  # notification was trying to get you to look.
  age.secrets = lib.optionalAttrs (!thinClient) {
    ntfy_topic.rekeyFile = ../../../secrets/store/ntfy_topic.age;
  };

  systemd.services.tailscale-autoconnect = lib.mkIf (!thinClient) {
    description = "Automatic connection to Tailscale";

    # make sure tailscale is running before trying to connect to tailscale
    after = [ "network-online.target" "tailscale.service" ];
    wants = [ "network-online.target" "tailscale.service" ];
    wantedBy = [ "multi-user.target" ];

    # set this service as a oneshot job
    serviceConfig.Type = "oneshot";

    # have the job run this shell script
    script = with pkgs; ''
      # wait for tailscaled to settle
      sleep 2

      # check if we are already authenticated to tailscale
      status="$(${tailscale}/bin/tailscale status -json | ${jq}/bin/jq -r .BackendState)"
      if [ $status = "Running" ]; then # if so, then do nothing
          exit 0
      fi

      # otherwise authenticate with tailscale
      # TODO: make the ssh key configurable
      ${tailscale}/bin/tailscale up --ssh &
      tail_pid=$!
      sleep 2
      auth_url="$(${tailscale}/bin/tailscale status -json | ${jq}/bin/jq -r .AuthURL)"
      kill $tail_pid

      NTFY_TOKEN=$(cat ${config.age.secrets.ntfy_topic.path})

      # check if this is a dummy value from rekey by checking the length of the token (it should be shorter than 20 characters)
      NTFY_TOKEN_LENGTH="$(echo -n "$NTFY_TOKEN" | wc -c)"
      if [ $NTFY_TOKEN_LENGTH -gt 20 ]; then
          echo There is no ntfy token set, skipping notification
          echo "Auth URL: $auth_url"
          exit 0
      fi

      echo "Sending notification to ntfy channel $auth_url"
      echo "NTFY_TOKEN" $NTFY_TOKEN
      ${curl}/bin/curl -H "X-Title: Tailscale Login: $HOSTNAME" \
          -H "X-Priority: 4" \
          -H "X-Actions: action=view, label=Open URL, url=$auth_url, clear=true" \
          -H "X-Click: $auth_url" \
          -H "X-Icon: https://tailscale.com/favicon.ico" \
          -d "There has been a Request to login to your tailscale network: $auth_url" \
          https://ntfy.sh/$NTFY_TOKEN
    '';
  };
  # Network manager should not manage tailscale0 interface
  # It does not bring up the wireguard interface properly when running nix-rebuild switch
  # see issue https://github.com/NixOS/nixpkgs/issues/180175
  systemd.services.NetworkManager-wait-online = {
    serviceConfig = {
      ExecStart = [ "" "${pkgs.networkmanager}/bin/nm-online -q" ];
      Restart = "on-failure";
      RestartSec = 1;
    };
    unitConfig.StartLimitIntervalSec = 0;
  };
}
