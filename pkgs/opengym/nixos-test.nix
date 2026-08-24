# End-to-end VM test for openGym: boots a machine running modules/nixos/opengym.nix
# against a stub OpenID Connect provider, and drives a real sign-in through nginx.
#
# This is the only place the OIDC patch in ./oidc.patch is actually executed as a
# whole flow — the packages build fine with a broken auth path, so a build alone
# proves very little. It also pins the two things that are easy to regress by
# accident and silent when they do: the systemd hardening, and the fact that
# nginx must overwrite (not append to) X-Forwarded-For, because openGym reads the
# first entry of that header as the caller's address for its audit log.
#
# Run with: nix build .#opengym-test -L   (needs KVM)
{
  lib,
  testers,
  python3,
  writeText,
  writeShellScriptBin,
  opengym-api,
  opengym-web,
  opengym-media,
}:
let
  host = "gym.test";
  idpHost = "idp.test";
  idpPort = 9999;
  issuer = "http://${idpHost}:${toString idpPort}/application/o/opengym/";
  clientSecret = "test-client-secret";

  # A minimal but honest identity provider: it checks the client secret, checks
  # the PKCE code_verifier against the challenge it was sent, and echoes the
  # nonce back inside the id_token. Those are exactly the three things a broken
  # client implementation gets wrong, so a stub that skipped them would pass on
  # code that no real provider would accept.
  # Plain writeText rather than writePython3Bin: the latter runs flake8 over
  # this, and a lint opinion about line length has no business failing a VM test.
  idpScript = writeText "opengym-test-idp.py" ''
    import base64
    import hashlib
    import json
    import time
    import urllib.parse
    from http.server import BaseHTTPRequestHandler, HTTPServer

    ISSUER = "${issuer}"
    CLIENT_ID = "opengym"
    CLIENT_SECRET = "${clientSecret}"
    GROUPS_FILE = "/run/idp-groups"
    SUB_FILE = "/run/idp-sub"
    NAME_FILE = "/run/idp-name"

    codes = {}


    def b64u(raw):
        return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


    def groups():
        try:
            with open(GROUPS_FILE) as fh:
                return [g for g in fh.read().split() if g]
        except FileNotFoundError:
            return []


    def read(path, fallback):
        try:
            with open(path) as fh:
                return fh.read().strip() or fallback
        except FileNotFoundError:
            return fallback


    class Handler(BaseHTTPRequestHandler):
        def reply(self, code, obj, headers=None):
            body = json.dumps(obj).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            for k, v in (headers or {}).items():
                self.send_header(k, v)
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):
            url = urllib.parse.urlparse(self.path)
            query = dict(urllib.parse.parse_qsl(url.query))

            if url.path.endswith("/.well-known/openid-configuration"):
                base = ISSUER.rstrip("/")
                return self.reply(200, {
                    "issuer": ISSUER,
                    "authorization_endpoint": base + "/authorize",
                    "token_endpoint": base + "/token",
                    "jwks_uri": base + "/jwks",
                    "response_types_supported": ["code"],
                    "subject_types_supported": ["public"],
                    "id_token_signing_alg_values_supported": ["RS256"],
                    "code_challenge_methods_supported": ["S256"],
                })

            if url.path.endswith("/authorize"):
                # No login form: this stands in for a provider where the user is
                # already authenticated and the application auto-consents.
                code = "code-" + b64u(hashlib.sha256(query["state"].encode()).digest())[:16]
                codes[code] = {
                    "nonce": query.get("nonce"),
                    "challenge": query.get("code_challenge"),
                    "method": query.get("code_challenge_method"),
                    "redirect_uri": query.get("redirect_uri"),
                }
                target = query["redirect_uri"] + "?" + urllib.parse.urlencode({
                    "code": code,
                    "state": query["state"],
                })
                self.send_response(302)
                self.send_header("Location", target)
                self.end_headers()
                return

            self.reply(404, {"error": "not found"})

        def do_POST(self):
            length = int(self.headers.get("Content-Length", 0))
            form = dict(urllib.parse.parse_qsl(self.rfile.read(length).decode()))

            if form.get("client_id") != CLIENT_ID or form.get("client_secret") != CLIENT_SECRET:
                return self.reply(400, {"error": "invalid_client"})

            entry = codes.pop(form.get("code", ""), None)
            if entry is None:
                return self.reply(400, {"error": "invalid_grant"})

            # PKCE. A client that forgot to carry the verifier across the
            # redirect, or hashed it wrong, dies right here.
            if entry["method"] != "S256":
                return self.reply(400, {"error": "invalid_request", "error_description": "expected S256"})
            verifier = form.get("code_verifier", "")
            expected = b64u(hashlib.sha256(verifier.encode()).digest())
            if expected != entry["challenge"]:
                return self.reply(400, {"error": "invalid_grant", "error_description": "PKCE mismatch"})

            if form.get("redirect_uri") != entry["redirect_uri"]:
                return self.reply(400, {"error": "invalid_grant", "error_description": "redirect_uri mismatch"})

            now = int(time.time())
            header = {"alg": "RS256", "typ": "JWT", "kid": "test"}
            payload = {
                "iss": ISSUER,
                "aud": CLIENT_ID,
                "sub": read(SUB_FILE, "stable-subject-0001"),
                "exp": now + 300,
                "iat": now,
                "nonce": entry["nonce"],
                "name": read(NAME_FILE, "Test User"),
                "preferred_username": "testuser",
                "email": "test@example.com",
                "groups": groups(),
            }
            # openGym does not verify the JWS (the token came straight from this
            # endpoint over an authenticated channel), so the signature segment
            # only has to be present and well-formed.
            id_token = ".".join([
                b64u(json.dumps(header).encode()),
                b64u(json.dumps(payload).encode()),
                b64u(b"not-a-real-signature"),
            ])
            self.reply(200, {
                "access_token": "test-access-token",
                "token_type": "Bearer",
                "expires_in": 300,
                "id_token": id_token,
            }, {"Cache-Control": "no-store"})

        def log_message(self, fmt, *args):
            print("idp: " + (fmt % args))


    HTTPServer(("0.0.0.0", ${toString idpPort}), Handler).serve_forever()
  '';
  passkeyScript = writeText "opengym-passkey-client.py" ''
    # A software WebAuthn authenticator, enough of one to register and use a passkey
    # against openGym. This exists because the passkey path is the app's original and
    # still-supported way in, and the OIDC patch touches session handling that both
    # paths share -- a test suite that only drove SSO would not notice if adding it
    # broke the thing it was added beside.
    #
    # It is a real client, not a stub: ES256 keys, a CBOR "none" attestation object,
    # and a genuine ECDSA signature over authenticatorData || SHA256(clientDataJSON).
    # @simplewebauthn/server verifies all of that on the other end, so anything wrong
    # here fails loudly rather than passing vacuously.
    import base64
    import hashlib
    import json
    import os
    import sys
    import urllib.request

    import cbor2
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec

    AAGUID = b"\x00" * 16


    def b64u_enc(raw):
        return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


    def b64u_dec(txt):
        return base64.urlsafe_b64decode(txt + "=" * (-len(txt) % 4))


    def post(base, path, payload, cookie=None):
        body = json.dumps(payload).encode()
        req = urllib.request.Request(base + path, data=body, method="POST")
        req.add_header("Content-Type", "application/json")
        if cookie:
            req.add_header("Cookie", "gymsid=" + cookie)
        with urllib.request.urlopen(req) as resp:
            set_cookie = resp.headers.get("Set-Cookie", "")
            new_cookie = None
            if set_cookie.startswith("gymsid="):
                new_cookie = set_cookie.split(";")[0].split("=", 1)[1]
            return json.loads(resp.read()), (new_cookie or cookie)


    def get(base, path, cookie=None):
        req = urllib.request.Request(base + path)
        if cookie:
            req.add_header("Cookie", "gymsid=" + cookie)
        with urllib.request.urlopen(req) as resp:
            return json.loads(resp.read())


    def client_data(kind, challenge, origin):
        # Field order is irrelevant to the server (it parses the JSON), but the exact
        # bytes matter: they are what gets hashed and signed.
        return json.dumps(
            {"type": kind, "challenge": challenge, "origin": origin, "crossOrigin": False},
            separators=(",", ":"),
        ).encode()


    def auth_data(rp_id, flags, counter, attested=b""):
        return (
            hashlib.sha256(rp_id.encode()).digest()
            + bytes([flags])
            + counter.to_bytes(4, "big")
            + attested
        )


    def cose_key(pub):
        nums = pub.public_numbers()
        return cbor2.dumps(
            {
                1: 2,    # kty: EC2
                3: -7,   # alg: ES256
                -1: 1,   # crv: P-256
                -2: nums.x.to_bytes(32, "big"),
                -3: nums.y.to_bytes(32, "big"),
            }
        )


    def do_register(base, origin, rp_id, name, code, statefile):
        opts, cookie = post(base, "/api/register/options", {"name": name, "code": code})
        o = opts["options"]

        key = ec.generate_private_key(ec.SECP256R1())
        cred_id = os.urandom(32)
        attested = (
            AAGUID + len(cred_id).to_bytes(2, "big") + cred_id + cose_key(key.public_key())
        )
        # UP | UV | AT
        ad = auth_data(rp_id, 0x45, 0, attested)
        cdj = client_data("webauthn.create", o["challenge"], origin)
        att_obj = cbor2.dumps({"fmt": "none", "attStmt": {}, "authData": ad})

        credential = {
            "id": b64u_enc(cred_id),
            "rawId": b64u_enc(cred_id),
            "type": "public-key",
            "authenticatorAttachment": "platform",
            "clientExtensionResults": {},
            "response": {
                "clientDataJSON": b64u_enc(cdj),
                "attestationObject": b64u_enc(att_obj),
                "transports": ["internal"],
            },
        }
        out, cookie = post(
            base, "/api/register/verify", {"cid": opts["cid"], "credential": credential}, cookie
        )
        with open(statefile, "w") as fh:
            json.dump(
                {
                    "cred_id": b64u_enc(cred_id),
                    "user_handle": o["user"]["id"],
                    "counter": 0,
                    "cookie": cookie,
                    "key": key.private_bytes(
                        serialization.Encoding.PEM,
                        serialization.PrivateFormat.PKCS8,
                        serialization.NoEncryption(),
                    ).decode(),
                },
                fh,
            )
        print(json.dumps(out))


    def do_login(base, origin, rp_id, statefile):
        with open(statefile) as fh:
            st = json.load(fh)
        key = serialization.load_pem_private_key(st["key"].encode(), password=None)

        opts, cookie = post(base, "/api/login/options", {})
        o = opts["options"]

        st["counter"] += 1
        ad = auth_data(rp_id, 0x05, st["counter"])          # UP | UV, no attested data
        cdj = client_data("webauthn.get", o["challenge"], origin)
        signature = key.sign(ad + hashlib.sha256(cdj).digest(), ec.ECDSA(hashes.SHA256()))

        credential = {
            "id": st["cred_id"],
            "rawId": st["cred_id"],
            "type": "public-key",
            "authenticatorAttachment": "platform",
            "clientExtensionResults": {},
            "response": {
                "clientDataJSON": b64u_enc(cdj),
                "authenticatorData": b64u_enc(ad),
                "signature": b64u_enc(signature),
                "userHandle": st["user_handle"],
            },
        }
        out, cookie = post(
            base, "/api/login/verify", {"cid": opts["cid"], "credential": credential}, cookie
        )
        st["cookie"] = cookie
        with open(statefile, "w") as fh:
            json.dump(st, fh)
        print(json.dumps(out))


    def do_me(base, statefile):
        with open(statefile) as fh:
            st = json.load(fh)
        print(json.dumps(get(base, "/api/me", st["cookie"])))


    if __name__ == "__main__":
        cmd = sys.argv[1]
        if cmd == "register":
            do_register(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5], sys.argv[6], sys.argv[7])
        elif cmd == "login":
            do_login(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5])
        elif cmd == "me":
            do_me(sys.argv[2], sys.argv[3])
        else:
            raise SystemExit("unknown command " + cmd)
  '';

  # The authenticator needs CBOR and an ECDSA implementation; everything else it
  # uses is stdlib.
  passkey = writeShellScriptBin "passkey" ''
    exec ${python3.withPackages (ps: [ps.cbor2 ps.cryptography])}/bin/python3 ${passkeyScript} "$@"
  '';
in
testers.runNixOSTest {
  name = "opengym";

  nodes.machine = {pkgs, ...}: {
    virtualisation.memorySize = 2048;
    virtualisation.diskSize = 4096;

    # Both names point at the machine itself; the split exists so the app and
    # the provider are genuinely different origins, as they are in production.
    networking.hosts."127.0.0.1" = [host idpHost];

    imports = [../../modules/nixos/opengym.nix];

    services.bcnelson.opengym = {
      enable = true;
      inherit host;
      # Passed explicitly: a test node's `pkgs` is plain nixpkgs and does not
      # carry this repo's `additions` overlay, so the module's defaults would
      # not resolve here.
      package = opengym-api;
      webPackage = opengym-web;
      mediaPackage = opengym-media;
      # No ACME in a VM with no DNS and no reachable CA.
      useACME = false;
      dataDir = "/var/lib/opengym";
      environmentFile = "/etc/opengym-secret.env";
      oidc = {
        enable = true;
        inherit issuer;
        clientId = "opengym";
        name = "TestIdP";
        adminGroups = ["service_admins"];
      };
    };

    # Stands in for the agenix-template file on romeo. It has to exist before the
    # unit starts rather than be written by an ExecStartPre: systemd reads
    # EnvironmentFile= first, so a secret created in start-pre is always too late
    # -- the unit fails with "Failed to load environment files" and restart-loops.
    # Unlike the real thing this is store-backed and world-readable, which is
    # fine for a throwaway VM and wrong for anything else.
    environment.etc."opengym-secret.env" = {
      text = "OIDC_CLIENT_SECRET=${clientSecret}\n";
      mode = "0400";
    };

    systemd.services.opengym-test-idp = {
      description = "Stub OpenID Connect provider";
      wantedBy = ["multi-user.target"];
      before = ["opengym-api.service"];
      serviceConfig = {
        ExecStartPre = "${pkgs.writeShellScript "idp-groups" ''
          echo 'service_admins household' > /run/idp-groups
        ''}";
        ExecStart = "${lib.getExe python3} ${idpScript}";
        Restart = "on-failure";
      };
    };

    environment.systemPackages = [pkgs.curl pkgs.jq passkey];
  };

  testScript = ''
    import json

    machine.wait_for_unit("opengym-test-idp.service")
    machine.wait_for_unit("opengym-api.service")
    machine.wait_for_unit("nginx.service")
    machine.wait_for_open_port(${toString idpPort})
    machine.wait_for_open_port(80)

    def api(path, *args):
        return machine.succeed(f"curl -sS {' '.join(args)} http://${host}{path}")

    with subtest("the API is only reachable through nginx"):
        # HOST=127.0.0.1 in the unit. If this ever regresses to a wildcard bind,
        # the port is exposed to the whole network with none of nginx's header
        # scrubbing in front of it.
        #
        # wait_for_open_port first: the unit is Type=simple, so systemd calls it
        # active the moment it forks, well before node has bound anything.
        machine.wait_for_open_port(3111, addr="127.0.0.1")
        table = machine.succeed("ss -ltnH")
        listeners = [ln for ln in table.splitlines() if ":3111" in ln]
        assert listeners, f"nothing listening on 3111:\n{table}"
        assert all("127.0.0.1:3111" in ln for ln in listeners), \
            "API is listening beyond loopback:\n" + "\n".join(listeners)

    with subtest("the systemd sandbox is intact"):
        # The unit is heavily locked down on purpose; this catches a directive
        # being dropped or a future nixpkgs default relaxing something. The
        # threshold is deliberately just above the current score (1.1) rather
        # than a round number, so a real regression cannot hide under slack.
        out = machine.succeed("systemd-analyze security opengym-api.service | tail -1")
        exposure = float(out.split("exposure level for opengym-api.service:")[1].split()[0])
        assert exposure <= 1.5, f"hardening regressed: exposure {exposure} > 1.5\n{out}"

    with subtest("nginx serves the app, its assets and the exercise media"):
        index = api("/")
        assert "<title>openGym</title>" in index, index
        # SPA fallback: a client-side route must still return the shell.
        assert "<title>openGym</title>" in api("/plan/r/whatever")
        machine.succeed("curl -sSf http://${host}/img/0001-2gPfomN.jpg -o /tmp/i.jpg")
        machine.succeed("test -s /tmp/i.jpg")
        machine.succeed("curl -sSf http://${host}/gif/0001-2gPfomN.gif -o /tmp/i.gif")
        machine.succeed("test -s /tmp/i.gif")

    with subtest("the instance advertises SSO and nothing else"):
        cfg = json.loads(api("/api/config"))
        assert cfg == {"invite_only": True, "allow_guest": False, "oidc": "TestIdP"}, cfg

    with subtest("a full OIDC sign-in creates a profile and a session"):
        # -L walks the whole dance: /api/oidc/start -> the provider's authorize
        # endpoint -> back to /api/oidc/callback -> the app. The cookie jar is
        # what carries the anti-CSRF state cookie and then the session.
        machine.succeed("curl -sS -L -c /tmp/jar -b /tmp/jar http://${host}/api/oidc/start -o /dev/null")
        me = json.loads(machine.succeed("curl -sS -b /tmp/jar http://${host}/api/me"))
        assert me["user"]["name"] == "Test User", me
        # groups from the id_token include service_admins, and the module maps
        # that to the admin dashboard.
        assert me["user"]["admin"] is True, me
        health = json.loads(api("/api/health"))
        assert health["users"] == 1, health

    with subtest("the session actually carries per-user data"):
        machine.succeed(
            "curl -sSf -b /tmp/jar -X PUT http://${host}/api/data "
            "-H 'Content-Type: application/json' "
            """-d '{"state":{"workouts":[{"d":"2026-08-23"}]}}' -o /dev/null"""
        )
        data = json.loads(machine.succeed("curl -sS -b /tmp/jar http://${host}/api/data"))
        assert data["state"]["workouts"][0]["d"] == "2026-08-23", data
        # ...and that an unauthenticated caller gets none of it.
        machine.succeed("test 401 = \"$(curl -sS -o /dev/null -w %{http_code} http://${host}/api/data)\"")

    with subtest("signing in again reuses the profile rather than duplicating it"):
        machine.succeed("curl -sS -L -c /tmp/jar2 -b /tmp/jar2 http://${host}/api/oidc/start -o /dev/null")
        health = json.loads(api("/api/health"))
        assert health["users"] == 1, f"sub is not being used as a stable key: {health}"

    with subtest("losing the admin group takes admin away at the next sign-in"):
        machine.succeed("echo household > /run/idp-groups")
        machine.succeed("curl -sS -L -c /tmp/jar3 -b /tmp/jar3 http://${host}/api/oidc/start -o /dev/null")
        me = json.loads(machine.succeed("curl -sS -b /tmp/jar3 http://${host}/api/me"))
        assert me["user"]["admin"] is False, f"admin survived removal from the group: {me}"

        # Put the group back and sign in again on the original session. This is
        # not just cleanup: it is the re-promotion half of the same behaviour,
        # and /tmp/jar has to be an admin session again for the invite below.
        machine.succeed("echo 'service_admins household' > /run/idp-groups")
        machine.succeed("curl -sS -L -c /tmp/jar -b /tmp/jar http://${host}/api/oidc/start -o /dev/null")
        me = json.loads(machine.succeed("curl -sS -b /tmp/jar http://${host}/api/me"))
        assert me["user"]["admin"] is True, f"admin was not restored with the group: {me}"

    with subtest("passkey signup is refused without an invite"):
        # INVITE_ONLY=1 is what stops the passkey form being a way around the
        # identity provider, so it is worth pinning rather than assuming.
        machine.fail(
            'passkey register http://${host} http://${host} ${host} Nobody "" /tmp/nope.json'
        )
        health = json.loads(api("/api/health"))
        assert health["users"] == 1, health

    with subtest("an SSO admin can issue an invite, and a passkey can use it"):
        # The realistic path: whoever signed in through SSO and landed in the
        # admin group hands out a code, and that code buys exactly one profile.
        invite = json.loads(machine.succeed(
            "curl -sS -b /tmp/jar -X POST http://${host}/api/admin/invites/new "
            "-H 'Content-Type: application/json' -d '{\"note\":\"vm test\"}'"
        ))
        code = invite["invite"]["code"]

        # A real WebAuthn client: ES256 key, CBOR 'none' attestation, and a
        # genuine ECDSA assertion. @simplewebauthn/server verifies all of it.
        out = json.loads(machine.succeed(
            f"passkey register http://${host} http://${host} ${host} 'Passkey Person' {code} /tmp/pk.json"
        ))
        assert out["user"]["name"] == "Passkey Person", out
        assert out["user"]["admin"] is False, out
        health = json.loads(api("/api/health"))
        assert health["users"] == 2, health

        # ...and the code is single-use.
        machine.fail(
            f"passkey register http://${host} http://${host} ${host} 'Gatecrasher' {code} /tmp/nope.json"
        )

    with subtest("a passkey signs in and gets its own data, not the SSO user's"):
        out = json.loads(machine.succeed(
            "passkey login http://${host} http://${host} ${host} /tmp/pk.json"
        ))
        assert out["user"]["name"] == "Passkey Person", out
        me = json.loads(machine.succeed("passkey me http://${host} /tmp/pk.json"))
        assert me["user"]["name"] == "Passkey Person", me
        # The SSO user wrote a workout earlier; this profile must not see it.
        pk_id = me["user"]["id"]
        sso_id = json.loads(machine.succeed("curl -sS -b /tmp/jar http://${host}/api/me"))["user"]["id"]
        assert pk_id != sso_id, "passkey and SSO profiles collapsed into one"

    with subtest("SSO adopts a matching passkey profile instead of duplicating it"):
        # This is the migration path an existing instance depends on: a new
        # subject whose display name matches an account that has no subject yet
        # is linked to it, rather than silently starting from an empty history.
        machine.succeed("echo linked-subject-0002 > /run/idp-sub")
        machine.succeed("echo 'Passkey Person' > /run/idp-name")
        machine.succeed("curl -sS -L -c /tmp/jar4 -b /tmp/jar4 http://${host}/api/oidc/start -o /dev/null")
        me = json.loads(machine.succeed("curl -sS -b /tmp/jar4 http://${host}/api/me"))
        assert me["user"]["id"] == pk_id, f"expected the existing profile {pk_id}, got {me}"
        health = json.loads(api("/api/health"))
        assert health["users"] == 2, f"linking created a duplicate profile: {health}"

        # Restore the default identity for the checks that follow.
        machine.succeed("echo stable-subject-0001 > /run/idp-sub")
        machine.succeed("echo 'Test User' > /run/idp-name")

    with subtest("a callback cannot be replayed or forged"):
        # No such state was ever issued.
        body = machine.succeed(
            "curl -sS 'http://${host}/api/oidc/callback?code=x&state=forged'"
        )
        assert "Sign-in failed" in body, body

        # A state we did issue, but presented by a browser that never started the
        # flow — this is the login-CSRF case, where an attacker hands a victim
        # their own callback URL to land them in the attacker's account.
        loc = machine.succeed(
            "curl -sS -o /dev/null -D - http://${host}/api/oidc/start | "
            "grep -i '^location:' | sed 's/.*state=//;s/&.*//' | tr -d '\\r\\n'"
        )
        body = machine.succeed(f"curl -sS 'http://${host}/api/oidc/callback?code=x&state={loc}'")
        assert "did not start in this browser" in body, body

    with subtest("nginx does not let a client forge its own address in the audit log"):
        # openGym reads the FIRST entry of X-Forwarded-For, and CF-Connecting-IP
        # ahead of it. nginx must overwrite both. NixOS's recommendedProxySettings
        # would append instead, which is why the module turns it off.
        machine.succeed(
            "curl -sS -o /dev/null "
            "-H 'X-Forwarded-For: 203.0.113.99' "
            "-H 'CF-Connecting-IP: 198.51.100.7' "
            "'http://${host}/api/oidc/callback?code=x&state=spoof-probe'"
        )
        audit = machine.succeed("tail -1 /var/lib/opengym/audit.log")
        entry = json.loads(audit)
        assert "203.0.113" not in audit and "198.51.100" not in audit, \
            f"client-supplied address reached the audit log: {audit}"
        assert entry["ip"] == "127.0.0.0/24", entry

    with subtest("state survives a restart"):
        machine.succeed("systemctl restart opengym-api.service")
        machine.wait_for_unit("opengym-api.service")
        machine.wait_until_succeeds("curl -sSf http://${host}/api/health -o /dev/null", timeout=30)
        health = json.loads(api("/api/health"))
        assert health["users"] == 2, health
        # The session cookie is signed with the on-disk secret, so a session that
        # survives a restart proves the secret was persisted rather than
        # regenerated into a fresh data directory.
        me = json.loads(machine.succeed("curl -sS -b /tmp/jar http://${host}/api/me"))
        assert me["user"]["name"] == "Test User", me
  '';
}
