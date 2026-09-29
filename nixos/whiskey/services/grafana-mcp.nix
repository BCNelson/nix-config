{ config, pkgs, ... }:
let
  port = 8765;
  host = "${config.networking.hostName}.b.nel.family";

  # The public URL, not 127.0.0.1:2342 - enforce_domain 301s any request whose
  # Host is not grafana.b.nel.family.
  grafanaUrl = "https://${config.services.grafana.settings.server.domain}";

  # Read-only toolsets that cover debugging: querying Prometheus and Loki,
  # finding dashboards, and reading alert rules.
  enabledTools = [
    "search"
    "datasource"
    "prometheus"
    "loki"
    "alerting"
    "dashboard"
    "folder"
    "navigation"
    "annotations"
  ];

  # Grafana has no file provisioning for service accounts, so this makes sure a
  # working token exists on every start: it reuses the stored one while Grafana
  # still accepts it, and otherwise (first start, or after grafana.db is reset)
  # creates a Viewer service account and a fresh token using the local admin.
  start = pkgs.writeShellApplication {
    name = "grafana-mcp-start";
    runtimeInputs = with pkgs; [ coreutils curl jq ];
    text = ''
      grafana=${grafanaUrl}
      token_file="$STATE_DIRECTORY/token"

      token_works() {
        [ -s "$token_file" ] && curl -sf -o /dev/null \
          -H "Authorization: Bearer $(cat "$token_file")" "$grafana/api/datasources"
      }

      if ! token_works; then
        echo "grafana-mcp: no working token; creating one"
        # The local admin's password is not in Nix; it was written to this file
        # when grafana.db was last reset (2026-09-28).
        admin="admin:$(cat "$CREDENTIALS_DIRECTORY/grafana-admin-password")"
        api() { curl -sf -u "$admin" -H 'Content-Type: application/json' "$@"; }

        id=$(api "$grafana/api/serviceaccounts/search?query=mcp" \
          | jq -r '.serviceAccounts[] | select(.name == "mcp") | .id')
        if [ -z "$id" ]; then
          id=$(api -X POST "$grafana/api/serviceaccounts" \
            -d '{"name": "mcp", "role": "Viewer"}' | jq -r .id)
        fi

        umask 077
        api -X POST "$grafana/api/serviceaccounts/$id/tokens" \
          -d "{\"name\": \"mcp-$(date +%Y%m%d-%H%M%S)\"}" | jq -r .key > "$token_file"
        token_works
      fi

      GRAFANA_URL=$grafana
      GRAFANA_SERVICE_ACCOUNT_TOKEN=$(cat "$token_file")
      export GRAFANA_URL GRAFANA_SERVICE_ACCOUNT_TOKEN

      exec ${pkgs.mcp-grafana}/bin/mcp-grafana \
        --transport streamable-http \
        --address 0.0.0.0:${toString port} \
        --allowed-hosts ${host}:${toString port},${host} \
        --enabled-tools ${builtins.concatStringsSep "," enabledTools} \
        --disable-write
    '';
  };
in
{
  # Grafana's MCP server, shared by every agent on the tailnet (see .mcp.json at
  # the repo root). It carries no auth of its own; like Loki (:3100) and
  # Prometheus (:9001) it is reachable only over tailscale0, and it can only read.
  systemd.services.grafana-mcp = {
    description = "Grafana MCP server";
    after = [ "grafana.service" "nginx.service" "network-online.target" ];
    wants = [ "grafana.service" "network-online.target" ];
    wantedBy = [ "multi-user.target" ];

    serviceConfig = {
      ExecStart = "${start}/bin/grafana-mcp-start";
      DynamicUser = true;
      StateDirectory = "grafana-mcp";
      LoadCredential = [ "grafana-admin-password:/root/grafana-admin-password" ];
      # Grafana may still be starting at boot; keep retrying until it answers.
      Restart = "always";
      RestartSec = 10;
    };
  };

  networking.firewall.interfaces.tailscale0.allowedTCPPorts = [ port ];
}
