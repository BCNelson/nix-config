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
      for _ in $(seq 1 60); do
          auth_url="$(${tailscale}/bin/tailscale status -json | ${jq}/bin/jq -r '.AuthURL // empty')"
          [ -n "$auth_url" ] && break
          sleep 1
      done
      kill $tail_pid 2>/dev/null || true

      if [ -z "$auth_url" ]; then
          echo "tailscaled produced no auth URL after 60s, so there is nothing to send."
          echo "Authenticate this host manually with: tailscale up --ssh"
          exit 1
      fi

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
          echo "(or run: tailscale up --ssh)"
          exit 0
      fi

      # Deliberately does not echo the topic. It is an agenix secret, and the
      # journal is not a place to put one -- it was being printed in full on
      # every run.
      echo "Sending tailscale auth URL to ntfy: $auth_url"
      # -f so an HTTP error is a unit failure rather than a silent success. The
      # 400 above looked like a working notification from systemd's point of
      # view, which is how it went unnoticed.
      ${curl}/bin/curl -fsS -H "X-Title: Tailscale Login: $HOSTNAME" \
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
