{config, ...}:
# openGym — gym & body-weight tracker, behind authentik SSO.
#
# The service itself, its systemd hardening and its nginx vhost live in
# modules/nixos/opengym.nix, which is what the VM test (pkgs/opengym/nixos-test.nix)
# exercises. This file is only what is specific to romeo: where the data lives,
# which identity provider, and the secret.
let
  dataDirs = config.data.dirs;
in {
  ##########################################################################
  # Secrets
  #
  # The OIDC client secret is shared with authentik on whiskey — same rekeyFile
  # on both sides, so the two cannot drift. It reaches the service as an
  # EnvironmentFile, which systemd reads as root before dropping privileges, so
  # the file stays root-owned and the service never has rights to it on disk.
  ##########################################################################
  age.secrets.opengym-oauth-client-secret = {
    rekeyFile = ../../../secrets/store/shared/opengym_auth_client_secret.age;
    generator.script = "alnum";
  };

  age-template.files.opengym-env = {
    vars = {
      clientSecret = config.age.secrets.opengym-oauth-client-secret.path;
    };
    content = ''
      OIDC_CLIENT_SECRET=$clientSecret
    '';
  };

  services.bcnelson.opengym = {
    enable = true;
    host = "gym.nel.family";

    # Workout history and body-weight logs: small, entirely irreplaceable, and
    # not reconstructible from anywhere else. level3 is where the other
    # personal-record services (actual, homebox, fastenhealth) live and is
    # covered by the existing vault snapshot + borg jobs.
    dataDir = "${dataDirs.level3}/opengym";

    environmentFile = config.age-template.files.opengym-env.path;

    # Both doors that bypass the identity provider stay shut: no open passkey
    # signup form, and no "continue without account" guest mode. Together with
    # the OIDC block below, authentik is the only way in.
    inviteOnly = true;
    allowGuest = false;

    oidc = {
      enable = true;
      issuer = "https://auth.nel.family/application/o/opengym/";
      clientId = "opengym";
      name = "Authentik";
      # authentik has no dedicated groups scope; membership rides in the default
      # profile claim, so asking for profile is what makes adminGroups work.
      scopes = ["openid" "profile" "email"];
      adminGroups = ["service_admins"];
      # Access is already gated by the application's policy bindings in
      # authentik, so anyone who gets this far is meant to have a profile.
      autoProvision = true;
    };

    extraEnvironment = {
      # Push services want a way to contact whoever runs the server.
      VAPID_SUBJECT = "mailto:admin@nel.family";
    };
  };
}
