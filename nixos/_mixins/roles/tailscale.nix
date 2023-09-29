{ pkgs, ... }:

let
  sensitive = import ../../sensitive.nix;
in
{
  environment.systemPackages = with pkgs; [
    tailscale
    jq # Needed for parsing tailscale status in the setup script
  ];
  services.tailscale.enable = true;
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
      sleep 2
      auth_url="$(${tailscale}/bin/tailscale status -json | ${jq}/bin/jq -r .AuthURL)"
      kill $tail_pid

      echo "Sending notification to ntfy channel $auth_url"
      ${curl}/bin/curl -H "X-Title: Tailscale Login: $HOSTNAME" \
          -H "X-Priority: 4" \
          -H "X-Actions: action=view, label=Open URL, url=$auth_url, clear=true" \
          -H "X-Click: $auth_url" \
          -H "X-Icon: https://tailscale.com/favicon.ico" \
          -d "There has been a Request to login to your tailscale network: $auth_url" \
          https://ntfy.sh/${sensitive.ntfy_tailscale_topic}
    '';
  };
}
