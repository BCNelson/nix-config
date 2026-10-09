#!/usr/bin/env bash
# Run the spool-keys Secret Service integration tests against a THROWAWAY
# gnome-keyring on a PRIVATE D-Bus session bus. Never touches the user's real
# session bus, KWallet or keyring.
#
# Usage (inside the dev shell, from pkgs/spool):
#   crates/spool-keys/tests/secret-service-harness.sh [extra cargo test args]
#
# Only gnome-keyring is supported as the throwaway backend. oo7-daemon 0.6
# was tried and rejected: it binds its PAM listener at the hard-coded
# /run/user/$UID/oo7-pam.sock (ignoring XDG_RUNTIME_DIR), i.e. it escapes the
# sandbox, and `--login` left the new login collection locked.
#
# What it does:
#   1. mktemp a sandbox dir (spool-ss-test.XXXXXX); HOME, XDG_DATA_HOME,
#      XDG_CONFIG_HOME, XDG_CACHE_HOME, XDG_STATE_HOME and XDG_RUNTIME_DIR all
#      point inside it, so the keyring files and control socket are throwaway.
#   2. Starts a private dbus-daemon via `dbus-run-session --config-file` with a
#      config that listens only inside the sandbox and has NO service dirs
#      (nothing can be D-Bus-activated, e.g. the system's ksecretd).
#   3. Inside that bus, starts `gnome-keyring-daemon --foreground --unlock
#      --components=secrets` fed a test password on stdin (creates and
#      unlocks a fresh `login` keyring, which becomes the `default` alias).
#   4. Runs `cargo test -p spool-keys --test secret_service` with
#      SPOOL_SECRET_SERVICE_TESTS=1 plus markers the tests verify before
#      making any D-Bus call (bus address != the inherited one, bus socket
#      inside the sandbox, marker file matches).
set -euo pipefail

if [[ "${1:-}" != "--inner" ]]; then
  for tool in dbus-run-session dbus-send gnome-keyring-daemon cargo; do
    command -v "$tool" >/dev/null || { echo "missing $tool (use the dev shell)" >&2; exit 1; }
  done
  sandbox=$(mktemp -d -t spool-ss-test.XXXXXX)
  trap 'rm -rf "$sandbox"' EXIT
  chmod 700 "$sandbox"
  mkdir -m 700 -p "$sandbox"/{home,data,config,cache,state,run,bus}
  cat >"$sandbox/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$sandbox/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
  # Keep cargo's caches where they are; everything else goes to the sandbox.
  export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
  export SPOOL_TEST_PARENT_DBUS="${DBUS_SESSION_BUS_ADDRESS-}"
  export SPOOL_TEST_SANDBOX="$sandbox"
  env -u DBUS_SESSION_BUS_ADDRESS -u GNOME_KEYRING_CONTROL -u SSH_AUTH_SOCK \
    HOME="$sandbox/home" \
    XDG_DATA_HOME="$sandbox/data" XDG_CONFIG_HOME="$sandbox/config" \
    XDG_CACHE_HOME="$sandbox/cache" XDG_STATE_HOME="$sandbox/state" \
    XDG_RUNTIME_DIR="$sandbox/run" \
    dbus-run-session --config-file="$sandbox/bus.conf" -- "$0" --inner "$@"
  exit $?
fi
shift

# ---- inner: running on the private bus ---------------------------------
: "${SPOOL_TEST_SANDBOX:?}" "${DBUS_SESSION_BUS_ADDRESS:?}"
if [[ "$DBUS_SESSION_BUS_ADDRESS" == "${SPOOL_TEST_PARENT_DBUS-}" ]]; then
  echo "refusing: still on the inherited session bus" >&2
  exit 1
fi
case "$DBUS_SESSION_BUS_ADDRESS" in
  *"$SPOOL_TEST_SANDBOX/bus/"*) ;;
  *) echo "refusing: bus socket is not inside the sandbox" >&2; exit 1 ;;
esac
printf '%s' "$DBUS_SESSION_BUS_ADDRESS" >"$SPOOL_TEST_SANDBOX/.spool-test-sandbox"

export SPOOL_TEST_KEYRING_PASSWORD="spool-test-password"
printf '%s' "$SPOOL_TEST_KEYRING_PASSWORD" |
  gnome-keyring-daemon --foreground --unlock --components=secrets \
    >"$SPOOL_TEST_SANDBOX/daemon.log" 2>&1 &
gk=$!
trap 'kill $gk 2>/dev/null || true' EXIT

# Wait for org.freedesktop.secrets on the private bus.
for _ in $(seq 100); do
  if dbus-send --session --print-reply --dest=org.freedesktop.DBus / \
    org.freedesktop.DBus.NameHasOwner string:org.freedesktop.secrets 2>/dev/null |
    grep -q 'boolean true'; then
    break
  fi
  sleep 0.1
done

rc=0
SPOOL_SECRET_SERVICE_TESTS=1 cargo test -p spool-keys --test secret_service "$@" -- --test-threads=1 --nocapture || rc=$?
if [[ $rc -ne 0 ]]; then
  echo "---- gnome-keyring-daemon log (tail) ----" >&2
  tail -n 40 "$SPOOL_TEST_SANDBOX/daemon.log" >&2 || true
fi
exit $rc
