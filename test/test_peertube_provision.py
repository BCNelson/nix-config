"""Exercise provisioning failures and reconciliation without a live server."""

import importlib.util
import io
import json
from pathlib import Path
import tempfile
import textwrap
from types import SimpleNamespace
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import parse_qs

source = Path(__file__).resolve().parents[1] / "nixos/romeo/services/peertube-sso.py"
spec = importlib.util.spec_from_file_location("provision", source)
provision = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provision)


OIDC_PATH = "/nix/store/test-oidc/peertube-plugin-auth-openid-connect"


class ProvisionTests(unittest.TestCase):
    def run_provision(self, missing=False, denied=False, plugin_state=None, oidc_version=provision.OIDC_VERSION, oidc_uninstalled=False):
        calls = []
        versions = {provision.PLUGIN: oidc_version}
        if plugin_state is None:
            plugin_state = {} if missing else {
                provision.PLUGIN: {}, provision.TRANSCODING_PLUGIN: {},
            }

        def respond(req, timeout):
            calls.append(req)
            # Matches the real OAuth endpoint's Host guard, which rejects a
            # loopback Host even though the rest of the public API accepts it.
            self.assertEqual(req.get_header("Host"), "tube.nel.family")
            self.assertEqual(req.get_header("X-forwarded-proto"), "https")
            path = req.full_url.split("/api/v1")[1]
            if path == "/oauth-clients/local":
                body = {"client_id": "client", "client_secret": "client-secret"}
            elif path == "/users/token":
                form = parse_qs(req.data.decode())
                self.assertEqual(form["password"], ["root-secret"])
                if denied:
                    raise HTTPError(req.full_url, 401, "Unauthorized", {}, None)
                body = {"access_token": "test-token"}
            else:
                self.assertEqual(req.get_header("Authorization"), "Bearer test-token")
                body = {}
                if req.method == "GET":
                    name = path.split("/")[2]
                    if name not in plugin_state:
                        raise HTTPError(req.full_url, 404, "Not found", {}, None)
                    body = {"settings": plugin_state[name], "version": versions.get(name, "0.0.5"),
                            "uninstalled": oidc_uninstalled if name == provision.PLUGIN else False}
                elif req.method == "PUT":
                    plugin_state[path.split("/")[2]] = json.loads(req.data)["settings"]
                elif path == "/plugins/install":
                    payload = json.loads(req.data)
                    name = provision.PLUGIN if "path" in payload else payload["npmName"]
                    plugin_state.setdefault(name, {})
                    body = {"settings": plugin_state[name]}
                elif path == "/plugins/update":
                    self.assertEqual(json.loads(req.data), {"path": OIDC_PATH})
                    versions[provision.PLUGIN] = provision.OIDC_VERSION
                    body = {"settings": plugin_state[provision.PLUGIN], "version": provision.OIDC_VERSION}
            return io.BytesIO(json.dumps(body).encode())

        with tempfile.TemporaryDirectory() as tmp:
            password = Path(tmp) / "password"
            secret = Path(tmp) / "secret"
            password.write_text("root-secret\n")
            secret.write_text("oidc-secret\n")
            with patch.object(provision, "urlopen", side_effect=respond):
                if denied:
                    with self.assertRaises(HTTPError) as caught:
                        provision.configure("http://127.0.0.1:9001", password, secret, "tube.nel.family", OIDC_PATH)
                    caught.exception.close()
                else:
                    provision.configure("http://127.0.0.1:9001", password, secret, "tube.nel.family", OIDC_PATH)
        return calls

    def test_first_install_uses_managed_oidc_plugin_and_role_claim(self):
        calls = self.run_provision(missing=True)
        installs = [json.loads(r.data) for r in calls if r.full_url.endswith("/install")]
        self.assertEqual(installs, [
            {"path": OIDC_PATH},
            {"npmName": provision.TRANSCODING_PLUGIN, "pluginVersion": "0.0.5"},
        ])
        settings = [json.loads(r.data)["settings"] for r in calls if r.method == "PUT"]
        self.assertEqual(settings[0]["client-secret"], "oidc-secret")
        self.assertEqual(settings[0]["role-property"], "peertube_role")
        self.assertIn("peertube", settings[0]["scope"].split())
        profile = json.loads(settings[1]["transcoding-profiles"])["vod"][0]
        self.assertEqual(profile["encoderName"], "h264_vaapi")
        self.assertIn("-hwaccel_device /dev/dri/by-driver/i915-render", profile["inputOptions"])

    def test_existing_plugins_are_configured_without_reinstall(self):
        calls = self.run_provision()
        self.assertFalse(any(r.full_url.endswith("/install") for r in calls))
        self.assertEqual(sum(r.method == "PUT" for r in calls), 2)
        self.assertTrue(calls[-1].full_url.endswith("/users/revoke-token"))

    def test_second_run_does_not_resave_settings_or_duplicate_auth_methods(self):
        state = {}
        self.run_provision(plugin_state=state)
        calls = self.run_provision(plugin_state=state)
        self.assertFalse(any(r.method == "PUT" for r in calls))
        self.assertFalse(any(r.full_url.endswith("/install") for r in calls))
        self.assertTrue(calls[-1].full_url.endswith("/users/revoke-token"))

    def test_managed_drift_is_corrected_and_other_settings_are_preserved(self):
        state = {}
        self.run_provision(plugin_state=state)
        state[provision.PLUGIN]["role-property"] = "admin_role"
        state[provision.PLUGIN]["logout-redirect-uri"] = "https://tube.nel.family/"
        calls = self.run_provision(plugin_state=state)
        self.assertEqual(sum(r.method == "PUT" for r in calls), 1)
        self.assertEqual(state[provision.PLUGIN]["role-property"], "peertube_role")
        self.assertEqual(state[provision.PLUGIN]["logout-redirect-uri"], "https://tube.nel.family/")

    def test_existing_official_plugin_is_upgraded_without_losing_settings(self):
        state = {provision.PLUGIN: {"logout-redirect-uri": "https://tube.nel.family/"},
                 provision.TRANSCODING_PLUGIN: {}}
        calls = self.run_provision(plugin_state=state, oidc_version="1.1.0")
        updates = [r for r in calls if r.full_url.endswith("/plugins/update")]
        self.assertEqual(len(updates), 1)
        self.assertEqual(state[provision.PLUGIN]["logout-redirect-uri"], "https://tube.nel.family/")

    def test_failed_upgrade_is_reinstalled_and_settings_are_preserved(self):
        state = {provision.PLUGIN: {"logout-redirect-uri": "https://tube.nel.family/"},
                 provision.TRANSCODING_PLUGIN: {}}
        calls = self.run_provision(plugin_state=state, oidc_version="1.1.0", oidc_uninstalled=True)
        self.assertEqual(sum(r.full_url.endswith("/plugins/install") for r in calls), 1)
        self.assertFalse(any(r.full_url.endswith("/plugins/update") for r in calls))
        self.assertEqual(state[provision.PLUGIN]["logout-redirect-uri"], "https://tube.nel.family/")

    def test_failed_admin_login_does_not_install_or_change_settings(self):
        calls = self.run_provision(denied=True)
        self.assertEqual(len(calls), 2)


class RoleMappingTests(unittest.TestCase):
    def test_only_exact_service_admins_membership_grants_admin(self):
        blueprint = source.parents[2] / "whiskey/services/authentik/blueprints/peertube.yaml"
        expression = blueprint.read_text().split("      expression: |\n", 1)[1].split("\n  - model:", 1)[0]
        namespace = {}
        exec("def evaluate(request):\n" + textwrap.indent(textwrap.dedent(expression), "    "), namespace)
        for groups, expected in [
            ({"service_admins"}, 0),
            ({"service_admins", "household"}, 0),
            ({"household"}, 2),
            ({"extended_family"}, 2),
            ({"authentik Admins"}, 2),
            ({"service_admin"}, 2),
            (set(), 2),
        ]:
            with self.subTest(groups=groups):
                manager = SimpleNamespace(filter=lambda name: SimpleNamespace(exists=lambda: name in groups))
                request = SimpleNamespace(user=SimpleNamespace(ak_groups=manager))
                self.assertEqual(namespace["evaluate"](request), {"peertube_role": expected})


if __name__ == "__main__":
    unittest.main()
