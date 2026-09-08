"""Configure OIDC and GPU transcoding using PeerTube's local admin API."""

import json
from pathlib import Path
import sys
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

PLUGIN = "peertube-plugin-auth-openid-connect"
TRANSCODING_PLUGIN = "peertube-plugin-transcoding-profile-debug"


def configure(base_url, password_file, secret_file, public_host="tube.nel.family"):
    token = None

    def request(method, path, data=None, form=False):
        # PeerTube's local OAuth-client endpoint checks Host against its public
        # webserver configuration, even when the transport stays on loopback.
        headers = {"Host": public_host, "X-Forwarded-Proto": "https"}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        if data is not None:
            headers["Content-Type"] = (
                "application/x-www-form-urlencoded" if form else "application/json"
            )
            data = (urlencode(data) if form else json.dumps(data)).encode()
        req = Request(base_url + "/api/v1" + path, data, headers, method=method)
        with urlopen(req, timeout=180) as response:
            body = response.read()
            return json.loads(body) if body else None

    for attempt in range(60):
        try:
            client = request("GET", "/oauth-clients/local")
            break
        except HTTPError as error:
            if error.code < 500:
                raise
            error.close()
            if attempt == 59:
                raise RuntimeError("PeerTube did not become ready") from None
            time.sleep(5)
        except (URLError, TimeoutError):
            if attempt == 59:
                raise RuntimeError("PeerTube did not become ready") from None
            time.sleep(5)

    token = request("POST", "/users/token", {
        "client_id": client["client_id"],
        "client_secret": client["client_secret"],
        "grant_type": "password",
        "username": "root",
        "password": Path(password_file).read_text().strip(),
    }, form=True)["access_token"]

    try:
        def ensure_plugin(name, version):
            try:
                return request("GET", f"/plugins/{name}").get("settings") or {}
            except HTTPError as error:
                if error.code != 404:
                    raise
                error.close()
                request("POST", "/plugins/install", {
                    "npmName": name,
                    "pluginVersion": version,
                })
                return {}

        def reconcile_settings(name, current, desired):
            # Re-saving unchanged OIDC settings can register duplicate login
            # methods when discovery is unavailable. Keep reconciliation a no-op
            # when managed values match, and preserve other administrator options.
            if any(current.get(key) != value for key, value in desired.items()):
                request("PUT", f"/plugins/{name}/settings", {
                    "settings": {**current, **desired},
                })

        # OIDC 2.x requires PeerTube >= 8.3; Romeo currently pins 8.2.4.
        oidc_settings = ensure_plugin(PLUGIN, "1.1.0")

        reconcile_settings(PLUGIN, oidc_settings, {
            "auth-display-name": "Authentik",
            "discover-url": "https://auth.nel.family/application/o/peertube/",
            "client-id": "peertube",
            "client-secret": Path(secret_file).read_text().strip(),
            "scope": "openid email profile",
            "username-property": "preferred_username",
            "mail-property": "email",
            "display-name-property": "name",
            "signature-algorithm": "RS256",
            # Authentik application policies control access. No role claim means
            # regular User, never automatic administrator privileges.
            "role-property": "",
            "group-property": "",
            "allowed-group": "",
        })
        transcoding_settings = ensure_plugin(TRANSCODING_PLUGIN, "0.0.5")
        profiles = {
            "vod": [{
                "encoderName": "h264_vaapi",
                "profileName": "a380-vaapi",
                "inputOptions": [
                    "-hwaccel vaapi",
                    "-hwaccel_device /dev/dri/by-driver/i915-render",
                    "-hwaccel_output_format vaapi",
                ],
                "scaleFilter": {"name": "scale_vaapi"},
                "outputOptions": ["-pix_fmt vaapi", "-rc_mode CQP", "-global_quality 23"],
            }, {
                "encoderName": "aac",
                "profileName": "a380-vaapi",
                "outputOptions": ["-b:a 128k"],
            }],
            "live": [],
        }
        priorities = {
            "vod": [{"streamType": "video", "encoderName": "h264_vaapi", "priority": 1000}],
            "live": [],
        }
        reconcile_settings(TRANSCODING_PLUGIN, transcoding_settings, {
            "transcoding-profiles": json.dumps(profiles),
            "encoders-priorities": json.dumps(priorities),
        })
    finally:
        # Do not accumulate reusable administrator sessions on every boot.
        request("POST", "/users/revoke-token")
    print("PeerTube Authentik login and A380 transcoding configured")


if __name__ == "__main__":
    try:
        configure(*sys.argv[1:])
    except Exception as error:
        # Do not log response bodies, credentials, or authorization headers.
        status = f", HTTP {error.code}" if isinstance(error, HTTPError) else ""
        print(f"PeerTube plugin configuration failed ({type(error).__name__}{status})", file=sys.stderr)
        if isinstance(error, HTTPError):
            error.close()
        sys.exit(1)
