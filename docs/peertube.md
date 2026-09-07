# PeerTube on Romeo

PeerTube is served at https://tube.nel.family using the native NixOS module.
The app listens on `127.0.0.1:9001`; nginx provides TLS, uploads, streaming,
and websocket routes. PostgreSQL and Redis use local Unix sockets.

Public viewing and federation remain enabled. Public registration is disabled.
The Authentik blueprint permits sign-in for `household` and `extended_family`; the official
OIDC plugin creates regular User accounts on first login. SSO does not make
public videos private. SMTP is disabled, including email password resets.

## Deployment and sign-in

1. Apply the `porkbun_dns_record.tube_nel_family-CNAME` DNS addition through the
   normal Terraform workflow. LAN DNS is configured in Romeo's unbound module.
2. Deploy Whiskey's configuration to load the PeerTube Authentik blueprint,
   then deploy Romeo's configuration. Encrypted signing, root-password and OIDC
   secrets are included for the appropriate hosts using agenix-rekey naming.
3. `peertube-sso.service` waits for PeerTube and installs the official
   `peertube-plugin-auth-openid-connect` **1.1.0** if missing, then configures it
   through the local admin API. It also installs the official
   `peertube-plugin-transcoding-profile-debug` 0.0.5 and creates the `a380-vaapi`
   profile selected by Nix for uploaded videos. OIDC 1.1.0 supports PeerTube 8.2.4;
   plugin 2.x requires PeerTube 8.3 or newer. Initial installation needs outbound
   access to npm. Existing plugin installations are not automatically upgraded.
4. Use **Authentik** on the login page for normal access. For administration,
   log in as `root`; retrieve its password on Romeo with
   `sudo cat /run/agenix/peertube-admin-password`. Promote a family account from
   the admin UI if desired. SSO never grants administrator privileges itself.

The package suppresses upstream's first-start log of a managed root password.
The admin secret generator produces 40 characters (PeerTube limits login
passwords to 50). The credential used during initial deployment was rotated
through PeerTube's official reset-password CLI after this behavior was found.

The helper reconciles plugin settings on boot and revokes its temporary admin
session afterward. It can be run with
`sudo systemctl restart peertube-sso`. Its local root login must stay in sync
with the encrypted admin-password secret. `PT_INITIAL_ROOT_PASSWORD` only sets
the password when the database is first initialized: rotating the secret alone
does not reset an existing account. Update both together when rotating it.
Removing a user from Authentik's allowed groups blocks new SSO logins but does
not immediately revoke existing PeerTube sessions; disable that PeerTube account
when immediate revocation is required.

## Nix settings

Edit `nixos/romeo/services/peertube.nix` for the instance name, registration,
SMTP, storage, transcoding and other `services.peertube.settings` values.
Plugin settings are maintained in `peertube-sso.py`, and access policy in
`nixos/whiskey/services/authentik/blueprints/peertube.yaml`.
Settings saved through the PeerTube admin UI can override the Nix-generated
configuration. Keep a setting in one place to avoid surprises.

Choose the hostname before first deployment: PeerTube does not support changing
the instance hostname afterward.

## Storage and recovery

- Media, plugins and supporting files: `/mnt/vault/data/level3/peertube/storage`.
- PostgreSQL: local cluster, with daily logical dumps at 03:15 in
  `/mnt/vault/data/level2/peertube/database`.
- Runtime configuration: `/var/lib/peertube/config`, archived daily at 03:20 to
  `/mnt/vault/data/level2/peertube/config/config.tar.gz`.
- Signing and login secrets: encrypted in this repository. Preserve these
  together with the database, configuration and media backups.

Existing vault snapshots and Borg jobs cover levels 2 and 3. Database/config
dumps are daily and not an atomic snapshot of media; stop writes and take a
fresh dump for a coordinated migration. Restore the database dump into the
local PostgreSQL service and restore the media/configuration with PeerTube
stopped, preserving ownership. Redeploy the same secrets before restarting.

## Hardening and verification

The upstream NixOS module runs as `peertube`, drops all capabilities, prohibits
privilege escalation, uses `ProtectSystem=strict`, hides home directories and
other users' processes, isolates temporary files, protects kernel
interfaces, restricts namespaces and filters system calls. Writable exceptions
are PeerTube state/cache, the dedicated vault directory, and temporary files.
Node.js retains executable-memory support (`MemoryDenyWriteExecute=false`).
Romeo overrides `PrivateDevices` and `PrivateUsers` to allow Intel VA-API access
with the `render` supplementary group. `DevicePolicy=closed` and a single
`DeviceAllow` entry permit the A380's `/dev/dri/by-driver/i915-render` device.
The B580 and card/control nodes are not allowed by this service's device policy.
The existing Intel media driver is provided explicitly in the service environment.
The service also supplies pnpm for plugin installation, puts its data in the
writable cache, and permits `chown` in the syscall filter. Runtime audit logs
identified this single syscall as pnpm's requirement; the process retains no
capabilities to change ownership to another user.

The `a380-vaapi` profile uses hardware decoding/scaling and H.264 encoding for
uploaded videos, plus AAC audio. It does not configure GPU livestreaming.
Input formats the GPU cannot decode may fail and need a software profile;
switch `transcoding.profile` to `default` to use software transcoding.
Landscape and portrait uploads and protected HLS media were tested on the A380.
Hardware encoding quality and throughput have not been benchmarked.

The SSO helper is read-only, drops capabilities, denies executable writable
memory, and permits network traffic only to localhost. PeerTube itself retains
outbound network access for federation, imports, OIDC and plugin downloads.
There is no outbound proxy or LAN destination filtering; systemd's filesystem
sandbox does not prevent requests to other machines on the LAN.

## Runtime verification (2026-09-07)

- Applied the targeted Terraform DNS resource and obtained a valid TLS certificate.
- Saved Romeo's configuration as a persistent boot generation and restarted PeerTube and its
  sandboxed provisioning unit; both official plugins installed and reconciled.
- Uploaded private landscape and portrait clips, observed PeerTube's FFmpeg
  using `h264_vaapi` and the A380 device, and downloaded HLS media for ffprobe
  validation. Both produced 720p/360p renditions; portrait 360p was 360x640.
  Anonymous access was denied and all test videos were deleted.
- Ran both backup units, checked the configuration archive, and restored the
  database dump successfully into an isolated disposable PostgreSQL cluster.
- Verified that the device cgroup denies opening the B580 render node.
- Active `systemd-analyze security` exposure scores: PeerTube **1.8 OK**, helper
  **3.2 OK**. These measure systemd sandbox exposure, not application security.
- Whiskey's configuration builds successfully. Its deployment and an actual
  Authentik login still require the pending Tailscale SSH identity verification.
  PeerTube's login plugin is configured, but SSO is not yet operational.

The repeatable integration check is `test/peertube_smoke.py`. Run it on Romeo
as root with Python 3; it finds FFmpeg through the running service environment.
Set `SHAPE=720x1280` to exercise portrait uploads. It creates and deletes a private
synthetic video, checks actual GPU use, verifies protected HLS media, and tests
anonymous denial. A Python interruption or server failure can leave the private
test video behind; remove it through the administrator video list if needed.

Useful operational checks:

```sh
systemctl status peertube peertube-sso redis-peertube
systemd-analyze security peertube.service peertube-sso.service
systemctl start postgresqlBackup-peertube peertube-config-backup
```

References: [PeerTube configuration and security guidance](https://docs.joinpeertube.org/maintain/configuration),
[official systemd example](https://github.com/Chocobozzz/PeerTube/blob/v8.2.4/support/systemd/peertube.service),
[OIDC plugin](https://www.npmjs.com/package/peertube-plugin-auth-openid-connect/v/1.1.0).
