{ config, pkgs, ... }: {
  environment.systemPackages = with pkgs; [
    tailscale
    jq # Needed for parsing tailscale status in the setup script
  ];
  services.tailscale = {
    enable = true;
    package = pkgs.unstable.tailscale;
  };

  age.secrets.ntfy_topic.rekeyFile = ../../../secrets/store/ntfy_topic.age;

  systemd.services.tailscale-autoconnect = {
    description = "Automatic connection to Tailscale";

    # make sure tailscale is running before trying to connect to tailscale
    after = [ "network-online.target" "tailscale.service" ];
    wants = [ "network-online.target" "tailscale.service" ];
    wantedBy = [ "multi-user.target" ];

    # set this service as a oneshot job
    serviceConfig = {
      Type = "oneshot";
      # The script now blocks on `wait` until someone approves the login, so
      # the default 90s would kill it -- and killing it is precisely the bug
      # being fixed. Half an hour is long enough to walk to a phone and short
      # enough that an unattended host eventually gives up and reports failed
      # rather than sitting activating forever.
      TimeoutStartSec = "30min";
    };

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

      # Wait for the auth URL to exist rather than assuming two seconds is
      # enough. tailscaled has to reach the control plane and register a pending
      # node before .AuthURL is populated, and until then the field is present
      # but an empty string -- so a fixed sleep silently yields "".
      #
      # That is not a harmless miss. An empty url= makes ntfy reject the whole
      # request with 400 "Parameter URL is required for action view", so the
      # notification is not delivered at all and the URL reaches nobody. The
      # `// empty` also normalises a JSON null, which -r would otherwise hand
      # back as the literal string "null".
      auth_url=""
      for i in $(seq 1 60); do
          auth_url="$(${tailscale}/bin/tailscale status -json | ${jq}/bin/jq -r '.AuthURL // empty')"
          [ -n "$auth_url" ] && break
          sleep 1
      done

      if [ -z "$auth_url" ]; then
          echo "tailscaled produced no auth URL after 60s, so there is nothing to send."
          echo "Authenticate this host manually with: tailscale up --ssh"
          kill $tail_pid 2>/dev/null || true
          exit 1
      fi

      echo "auth URL appeared after ''${i}s"

      NTFY_TOKEN=$(cat ${config.age.secrets.ntfy_topic.path})

      # Do not POST to a placeholder. `agenix rekey --dummy` writes a whole
      # sentence explaining the secret was not rekeyed -- "This is a dummy
      # replacement value. ..." -- and treating that as a topic would push the
      # auth URL to a garbage channel and, worse, make the URL look delivered
      # when nobody can receive it. Match that sentinel directly rather than
      # inferring it, and keep the length test as a backstop: a real ntfy topic
      # is a short opaque token, so anything long is not one either way.
      NTFY_TOKEN_LENGTH="$(echo -n "$NTFY_TOKEN" | wc -c)"
      IS_DUMMY=0
      case "$NTFY_TOKEN" in
          *"dummy replacement value"*) IS_DUMMY=1 ;;
      esac
      if [ "$NTFY_TOKEN_LENGTH" -gt 20 ]; then IS_DUMMY=1; fi

      if [ "$IS_DUMMY" -eq 1 ]; then
          echo "ntfy topic is a placeholder, so no notification was sent."
          echo "Authenticate this host by opening:"
          echo "    $auth_url"
      else
          # Deliberately does not echo the topic. It is an agenix secret, and
          # the journal is not a place to put one -- it was being printed in
          # full on every run.
          echo "Sending tailscale auth URL to ntfy: $auth_url"
          # -f so curl treats an HTTP error as failure; the 400 that lost the
          # first notification looked like success without it. Handled rather
          # than fatal, though: this script runs under `set -e`, and failing
          # here would tear down the login below over a missed notification.
          if ! ${curl}/bin/curl -fsS -H "X-Title: Tailscale Login: $HOSTNAME" \
              -H "X-Priority: 4" \
              -H "X-Actions: action=view, label=Open URL, url=$auth_url, clear=true" \
              -H "X-Click: $auth_url" \
              -H "X-Icon: https://tailscale.com/favicon.ico" \
              -d "There has been a Request to login to your tailscale network: $auth_url" \
              https://ntfy.sh/$NTFY_TOKEN; then
              echo "WARNING: ntfy notification failed. The auth URL above is still valid."
          fi
      fi

      # Wait for the login instead of killing it. This used to `kill $tail_pid`
      # as soon as the URL was read, which is what left the first thin client
      # off the tailnet entirely: `tailscale up` is blocking *because* the
      # interactive login is bound to its LocalAPI connection, so killing the
      # client cancels the context tailscaled is registering under -- the
      # "context canceled" in its log -- and the pending node never completes.
      #
      # Being oneshot, this unit's children die with it, so returning early
      # would do the same thing by another route. Hence a single wait covering
      # both the notified and placeholder paths: the unit stays activating
      # until the URL is approved, and TimeoutStartSec ends it if nobody does.
      echo "Waiting for the login to be approved."
      wait $tail_pid
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
