# Configure OpenBao for Forgejo Actions. Idempotent: rerun it after editing.
#
# Runs as a privileged client: export BAO_TOKEN (the root token from
# `bao operator init`, or an admin token) before running it. BAO_ADDR is set
# system-wide on romeo.
#
# What it sets up:
#   ci/                 KV v2 mount. Secrets for a repo live at
#                       ci/<owner>/<repo>/<name>.
#   auth/jwt            trusts Forgejo's Actions OIDC issuer.
#   role forgejo-actions
#                       any job of a repo owned by one of $OWNERS that asked
#                       for audience $AUDIENCE. The token's identity alias is
#                       the repository ("owner/repo").
#   policy forgejo-actions
#                       read ci/<that repo>/*, templated on the alias, so one
#                       role serves every repo and none can read another's.
#
# The audit device is not here: OpenBao only accepts audit devices declared
# in its config (../openbao.nix).

: "${BAO_TOKEN:?export BAO_TOKEN (root or admin token) first}"

ISSUER="https://git.bcnelson.dev/api/actions"
AUDIENCE="https://bao.romeo.b.nel.family"
# Forgejo users/orgs whose repos may log in. JSON list.
OWNERS='["bcnelson"]'

if ! bao secrets list -format=json | jq -e '."ci/"' >/dev/null; then
  bao secrets enable -path=ci -version=2 kv
fi

if ! bao auth list -format=json | jq -e '."jwt/"' >/dev/null; then
  bao auth enable jwt
fi

bao write auth/jwt/config \
  oidc_discovery_url="$ISSUER" \
  bound_issuer="$ISSUER"

accessor=$(bao auth list -format=json | jq -r '."jwt/".accessor')

# The alias name is the `repository` claim (user_claim below), e.g.
# "bcnelson/nix-config", so this resolves to ci/data/bcnelson/nix-config/*.
bao policy write forgejo-actions - <<EOF
path "ci/data/{{identity.entity.aliases.${accessor}.name}}/*" {
  capabilities = ["read"]
}
path "ci/metadata/{{identity.entity.aliases.${accessor}.name}}/*" {
  capabilities = ["read", "list"]
}
EOF

jq -n \
  --arg aud "$AUDIENCE" \
  --argjson owners "$OWNERS" \
  '{
    role_type: "jwt",
    bound_audiences: [$aud],
    bound_claims: { repository_owner: $owners },
    user_claim: "repository",
    claim_mappings: {
      ref: "ref",
      ref_protected: "ref_protected",
      event_name: "event_name",
      workflow: "workflow",
      actor: "actor",
      sha: "sha"
    },
    token_policies: ["forgejo-actions"],
    token_ttl: "15m",
    token_max_ttl: "1h"
  }' | bao write auth/jwt/role/forgejo-actions -

echo "OpenBao configured: role forgejo-actions, secrets under ci/<owner>/<repo>/."
