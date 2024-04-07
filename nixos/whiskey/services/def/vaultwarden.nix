{ dataDirs, libx }:
let
    admin_token = libx.getSecret ../../sensitive.nix "bitwarden_admin_token";
    hibp_api_key = libx.getSecret ../../sensitive.nix "have_i_been_pwd_api_token";
    smtp_password = libx.getSecret ../../../sensitive.nix "smtp_password";
in
{
    vaultwarden = {
        image = "vaultwarden/server:latest";
        container_name = "vaultwarden";
        environment = [
            "ADMIN_TOKEN=${admin_token}"
            "HIBP_API_KEY=${hibp_api_key}"
            "WEBSOCKET_ENABLED=true"
            "LOG_LEVEL=info"
            "SIGNUPS_ALLOWED=false"
            "INVITATIONS_ALLOWED=true"
            "INVITATION_ORG_NAME=Nelson Family"
            "DOMAIN=https://vault.nel.family"
            "SMTP_HOST=smtp.migadu.com"
            "SMTP_FROM=admin@nel.family"
            "SMTP_FROM_NAME=VaultWarden"
            "SMTP_PORT=465"
            "SMTP_SSL=true"
            "SMTP_EXPLICIT_TLS=true"
            "SMTP_USERNAME=admin@nel.family"
            "SMTP_PASSWORD=${smtp_password}"
            "SMTP_TIMEOUT=15"
            "HELO_NAME=whiskey"
            "SMTP_DEBUG=false"
        ];
        volumes = [
            "${dataDirs.level1}/vaultwarden:/data"
        ];
        restart = "unless-stopped";
    };
}
