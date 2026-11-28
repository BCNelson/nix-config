# OpenBao for Forgejo Actions

OpenBao runs on romeo at `https://bao.romeo.b.nel.family` and is reachable
from the tailnet only (including job containers on romeo's runner). It is
configured in `nixos/romeo/services/openbao.nix`; auth, policies and mounts
come from `openbao-bootstrap` (`nixos/romeo/services/openbao/bootstrap.sh`).

## Storing a secret

Secrets for a repo live under `ci/<owner>/<repo>/`. A job of that repo can
read them and nothing else.

```
bao kv put -mount=ci bcnelson/my-repo/deploy token=...
```

## Reading it from a workflow

The job needs `enable-openid-connect: true`, and has to ask for the audience
`https://bao.romeo.b.nel.family`. Fork pull requests get no ID token, so they
cannot log in.

```yaml
jobs:
  deploy:
    runs-on: docker
    enable-openid-connect: true
    steps:
      - name: Fetch secrets from OpenBao
        run: |
          BAO=https://bao.romeo.b.nel.family
          jwt=$(curl -fsS -H "Authorization: Bearer $ACTIONS_ID_TOKEN_REQUEST_TOKEN" \
            "$ACTIONS_ID_TOKEN_REQUEST_URL&audience=$BAO" | jq -r .value)
          token=$(jq -n --arg jwt "$jwt" '{role: "forgejo-actions", jwt: $jwt}' \
            | curl -fsS -X POST --data @- "$BAO/v1/auth/jwt/login" | jq -r .auth.client_token)
          echo "::add-mask::$token"
          secret=$(curl -fsS -H "X-Vault-Token: $token" \
            "$BAO/v1/ci/data/${{ github.repository }}/deploy" | jq -r .data.data.token)
          echo "::add-mask::$secret"
          echo "DEPLOY_TOKEN=$secret" >> "$GITHUB_ENV"
```

`hashicorp/vault-action` should also work with `method: jwt`, `role:
forgejo-actions` and `jwtGithubAudience: https://bao.romeo.b.nel.family`,
but it has not been tested against Forgejo.

## Operations

- **Seal:** static key from agenix (`openbao_unseal_key`), so OpenBao unseals
  itself at boot. The raft data in backups is useless without that key, and
  the key is useless without the master keys.
- **Data:** `/mnt/vault/data/level1/openbao` (snapshots, syncoid to vor, borg).
- **Recovery keys and root token** come from the one-time `bao operator init`.
  Keep them in Bitwarden. Revoke the root token once you have an admin login.
- **Audit log:** stdout, so it's in romeo's journal (`journalctl -u openbao`).
- **More owners:** edit `OWNERS` in `bootstrap.sh` (e.g. add `home-first`)
  and rerun `openbao-bootstrap`.
