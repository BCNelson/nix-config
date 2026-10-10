# Spool

Spool is a clipboard history manager for KDE Plasma 6 on Wayland (with
backends for Hyprland and sway / other wlroots compositors). A small daemon
(`spoold`) watches and owns the Wayland clipboard through
`ext-data-control-v1`, filters what it sees through an ingest policy (secret
patterns, password-manager hints, size caps, MIME allowlist, excluded apps,
pause), stores history encrypted (SQLCipher + AEAD blob files + an encrypted
Tantivy search index), keeps the clipboard alive when the source app exits,
and serves local clients over a Unix socket. The data key lives in key slots
(KWallet / Secret Service, passphrase, FIDO2 security key). Meta+V opens a
resident Slint picker that searches as you type and pastes into the window
you came from.

Binaries: `spoold` (daemon), `spoolctl` (CLI), `spool-keyctl` (offline key
management), `spool-paster` (secret-free auto-paste helper) in the `spool`
package; `spool-picker` in its own `spool-picker` package (GPL-3.0-only via
Slint) so the daemon closure never pulls Slint.

The contract between crates and processes (wire formats, launch contracts,
locks, security model, compositor matrix) is in
[INTERFACES.md](INTERFACES.md).

## Crate map

| Crate | Kind | What it owns |
| --- | --- | --- |
| `crates/spool-proto` | lib | public socket + picker channel types, `Hello`, length-prefixed postcard framing |
| `crates/spool-crypto` | lib | data key / HKDF sub-keys, chunked AEAD files, sealed blobs |
| `crates/spool-core` | lib, no Wayland | item model, ingest `policy`, SQLite/SQLCipher `store`, TOML `config` |
| `crates/spool-keys` | lib | `keyslots.json`, Secret Service / session / passphrase / FIDO2 providers |
| `crates/spool-search` | lib | encrypted Tantivy index and query language |
| `crates/spool-wayland` | lib | Wayland clipboard thread (`spawn()` -> handle + events) |
| `crates/spool-kwin` | lib | KWin script loader, D-Bus service, active-window tracker |
| `crates/spool-paste` | lib + `spool-paster` | paste chords (KWin fake input / virtual keyboard), keycodes, foreign-toplevel focus |
| `crates/spool-compositor` | lib | compositor-neutral events and capabilities |
| `crates/spool-portal` | lib | hotkey via the XDG GlobalShortcuts portal |
| `crates/spool-hypr` | lib | Hyprland IPC (focus, cursor) |
| `crates/spoold` | bin | the daemon |
| `crates/spoolctl` | bin | `copy`, `current`, `show`, `pick [--raw]`, `edit`, `new [--mime M]`, `status [--json]`, `pause [--for SECS]`, `resume` |
| `crates/spool-keyctl` | bin + lib | `status`, `add`, `remove`, `rotate`, `recover`, `wipe` (see "Key management") |
| `crates/spool-picker` | bin | resident layer-shell picker, unlock panel, optional GPU renderer |

Nix files: [default.nix](default.nix) (`spool`), [picker.nix](picker.nix)
(`spool-picker`), [systemd-service.nix](systemd-service.nix) (the hardened
user unit, shared by the home-manager module and the VM test),
[nixos-test.nix](nixos-test.nix) (`spool-test`), [shell.nix](shell.nix) (dev
shell), [kwin-script/](kwin-script) (KWin script + desktop files).

## Installing

`pkgs/default.nix` exposes `spool`, `spool-picker` and `spool-test`. The
home-manager module (`modules/home-manager/spool.nix`, imported as
`outputs.homeModules.spool`) runs spoold as a hardened systemd user unit tied
to `graphical-session.target`, writes `~/.config/spool/config.toml` from its
options (`keyProvider`, `retention.*`, `primarySelection`, `excludedApps`,
`autoPaste`, `pasteTerminals`, `picker.*`, `editor.*`, `settings`), puts
`pickerPackage` on spoold's PATH and frees Plasma's Meta+V
(`disableKlipperShortcut`). The ready-made mixin
`home-manager/bcnelson/_mixins/programs/spool.nix` enables it; it is **not**
imported anywhere yet (rolling it out is a deliberate decision, see the
mixin's header).

Config keys are `deny_unknown_fields`: a key the daemon does not know makes
spoold refuse to start.

## Development

Dev shell (from the **repo root**; works without `NIX_PATH`, uses the flake's
pinned `nixpkgs-unstable`; provides cargo/rustc 1.98, clippy, rustfmt,
cargo-nextest, pkg-config, SQLCipher, libfido2, sway, wl-clipboard,
wayland-info, grim, wtype, wev, foot, nested KWin, dbus, gnome-keyring,
xdg-desktop-portal(-kde)):

```sh
nix develop --impure --expr 'let p = (builtins.getFlake "git+file:///home/bcnelson/nix-config").inputs.nixpkgs-unstable.legacyPackages.${builtins.currentSystem}; in p.callPackage ./pkgs/spool/shell.nix {}'
```

(Adjust the absolute path for another checkout.) For scripted use, capture
the environment once with `nix print-dev-env` and the same `--impure --expr`
(e.g. into a file you `source`), then run cargo directly.

**Pitfall: use `git+file://`, not `builtins.getFlake (toString ./.)`.** A
plain path flake copies the whole working tree into the store, including
the gitignored `pkgs/spool/target*` build directories (several GB), so
evaluation takes minutes. `git+file://` only copies tracked files. The same
rule applies to `nix build`: flakes only see files that are tracked (or at
least `git add`ed), so add new files before building.

Inside the shell, from `pkgs/spool`:

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all                      # rustfmt.toml: 2-space indent
```

Use one `CARGO_TARGET_DIR` under `pkgs/spool/target*` (gitignored) and
delete it when done. The E2E suites find `spool-picker`, `spool-paster`
and `spool-keyctl` **next to** the `spoold` they test, so build them into
the same target dir first: `cargo build --workspace --bins`.

### Running against a headless test compositor

Never point a development `spoold`, `wl-copy` or `wl-paste` at the live
desktop session: it would read and overwrite the real clipboard, and a
spoold that reaches the real session bus loads its KWin script into the real
KWin. Run a nested headless sway with its **own** `XDG_RUNTIME_DIR`, no
inherited session bus, a throwaway `SPOOL_STATE_DIR`, and `key_provider =
"session"` (memory only) unless you want to exercise the Secret Service.

```sh
# inside the dev shell
R=$(mktemp -d); chmod 700 "$R"
unset DBUS_SESSION_BUS_ADDRESS DISPLAY SWAYSOCK
env -u WAYLAND_DISPLAY XDG_RUNTIME_DIR="$R" WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman sway -c /dev/null >"$R/sway.log" 2>&1 &
sleep 2
export XDG_RUNTIME_DIR="$R" WAYLAND_DISPLAY=wayland-1
export SPOOL_STATE_DIR="$R/state" SPOOL_INSECURE_PATH_OK=1   # dev only
printf 'key_provider = "session"\n' >"$R/config.toml"; export SPOOL_CONFIG="$R/config.toml"
wayland-info | grep data_control          # sanity check
cargo run -p spoold -- --log-stderr &
echo hello | wl-copy; wl-paste -l; cargo run -p spoolctl -- current
kill %1 %2; rm -rf "$R"
```

`SPOOL_INSECURE_PATH_OK=1` is needed because the dev shell's PATH contains
non-root-owned directories; never set it in production. `SPOOL_SOCKET`
overrides the socket path for both binaries.

Verified 2026-10-08 (nixpkgs-unstable): sway 1.12 / wlroots 0.20.2 headless
advertises `ext_data_control_manager_v1` v1, `zwlr_data_control_manager_v1`
v2, `zwp_primary_selection_device_manager_v1` v1 and `wl_seat` v9.

Picker development harness (headless sway as above, plus `grim` and
`wtype`): `cargo build -p spool-picker --features dev --examples --bins`,
then `$CARGO_TARGET_DIR/debug/examples/mock_daemon --show` (stdin commands:
`show`, `hide`, `new`, `lock pass|fido|fidopin|both`, `touch`, `timeout`,
`nokey`, `bench N`, `stats`). It answers out of order (`MOCK_REORDER_MS`,
default 40) to exercise `seq` matching; fake passphrase `open sesame`, fake
PIN `1234`.

## Building the packages

From the repo root (stage new files first):

```sh
nix build .#spool .#spool-picker -L   # runs the unit tests (cargo test --workspace, minus the picker / plus the picker's own)
nix build .#spool-test -L             # NixOS VM test (needs KVM); .#spool-test.driverInteractive for a shell
```

The `src` filesets only include `Cargo.toml`, `Cargo.lock`, `crates/` (and
`kwin-script/` for `spool`), so README edits and target dirs never cause
rebuilds. The VM test runs the packaged spoold under exactly the unit the
home-manager module ships (headless sway session, throwaway gnome-keyring):
hardening properties, the sandbox probe, encrypted persistence across
restarts, the PATH audit, secrets not stored, session-only mode, and
the external editor started as a transient user unit outside the sandbox
(`spoolctl new` with a scripted editor).

## Testing

Everything automated runs isolated: private `XDG_RUNTIME_DIR`, private
`dbus-daemon` (no service activation), headless sway or nested virtual KWin,
throwaway gnome-keyring, temp state directories. Nothing touches the user's
session, clipboard, keyring, KWallet or security keys. Hyprland is never
run (it escaped nesting and tried to take over the real seat).

| Suite | Gate | Command (dev shell, `pkgs/spool`) |
| --- | --- | --- |
| unit + sandbox-safe integration (what `nix build` runs) | none | `cargo test --workspace` |
| headless sway: clipboard thread, spoold E2E (incl. real picker, `spoolctl pick`, auto-paste into `wev`, no stray Return after Enter, spool-keyctl refused while spoold runs), virtual-keyboard paste, foreign-toplevel focus | `SPOOL_WAYLAND_TESTS=1` | `cargo build --workspace --bins && SPOOL_WAYLAND_TESTS=1 cargo test --workspace` |
| same E2E plus search across restarts (debug search socket) | `SPOOL_WAYLAND_TESTS=1` + feature `test-hooks` | `SPOOL_WAYLAND_TESTS=1 cargo test -p spoold --features test-hooks --test e2e` |
| spoold persistence with a real Secret Service (`*_secret_service` E2E: restart, > 1 MiB blobs, no plaintext on disk, locked keyring at start -> merge after unlock, picker unlock panel) | `SPOOL_WAYLAND_TESTS=1` (starts its own private bus + gnome-keyring) | included in the two rows above |
| nested virtual KWin: script focus + shortcut + paste into `wev`, Dvorak keymap, fake input refused without / granted by a desktop file, non-KWin -> unsupported | `SPOOL_KWIN_TESTS=1` | `SPOOL_KWIN_TESTS=1 cargo test -p spool-paste --test nested_kwin` |
| nested KWin + spoold: script loads, Meta+V -> picker -> Enter -> Ctrl+V in `wev` via spool-paster, non-dumpable spoold refused | `SPOOL_KWIN_TESTS=1` + feature `test-hooks` | `SPOOL_KWIN_TESTS=1 cargo test -p spoold --features test-hooks --test nested_kwin` |
| spool-keys against a throwaway gnome-keyring | `SPOOL_SECRET_SERVICE_TESTS=1` (set by the harness) | `crates/spool-keys/tests/secret-service-harness.sh` |
| spool-keyctl against a throwaway gnome-keyring | `SPOOL_SECRET_SERVICE_TESTS=1` (harness) | `crates/spool-keyctl/tests/secret-service-harness.sh` |
| mock GlobalShortcuts portal on a private bus | `SPOOL_PORTAL_TESTS=1` | `SPOOL_PORTAL_TESTS=1 cargo test -p spool-portal --test mock_portal` |
| real portal stack: private bus + nested KWin + xdg-desktop-portal + xdp-kde + kglobalaccel (~10 s) | `SPOOL_PORTAL_KDE_TESTS=1` | `SPOOL_PORTAL_KDE_TESTS=1 cargo test -p spool-portal --test kde_portal` |
| real FIDO2 key (manual, touches needed) | `SPOOL_FIDO2_HW_TESTS=1` (+ `SPOOL_FIDO2_DEVICE`, `SPOOL_FIDO2_PIN`, `SPOOL_FIDO2_UV`) | `SPOOL_FIDO2_HW_TESTS=1 cargo test -p spool-keys --test fido2_hw -- --nocapture --test-threads=1` |
| search latency, 50k docs | `#[ignore]` | `cargo test -p spool-search --release --test latency -- --ignored --nocapture` |
| copy -> stored latency | `#[ignore]` + `SPOOL_WAYLAND_TESTS=1` | `SPOOL_WAYLAND_TESTS=1 cargo test -p spoold --test e2e -- --ignored --nocapture copy_latency` |
| spool-keyctl terminal prompts | `#[ignore]`, needs a pty | `(sleep 4; printf 'p\xc3\xa4ssX\x7f\rok\n') \| script -qec "cargo test -p spool-keyctl --lib tty_ -- --ignored" /dev/null` |
| NixOS VM | KVM | `nix build .#spool-test -L` (repo root) |

Other knobs: `SPOOL_SKIP_WAYLAND_TESTS` (force-skip), `SPOOL_E2E_SHOTS=DIR`
(keep the E2E / nested-KWin screenshots), `SPOOL_PICKER_GPU_ALLOW_SOFTWARE=1`
(let the GPU renderer run on llvmpipe; tests only). The nested-KWin suites
need `spool-paster` (and for spoold's, `spool-picker`) built next to spoold.

The harness scripts refuse to run outside their sandbox (bus address and
socket location are checked before any D-Bus call). Unit tests use fakes
for Wayland, key providers, the picker (socketpair), the paste sink and the
focus tracker; spool-keyctl's unit tests run the real Argon2id and inject a
crash after every rotation step.

## Manual checks on the real desktop

Things the isolated suites cannot cover. Do them on the real Plasma session
(or a spare VT / VM for Hyprland) after enabling the module, ideally first
with a throwaway state directory.

1. **KWin close-animation re-show delay.** On real KWin the picker's close
   animation delays an immediate re-show by ~190 ms (Meta+V, Esc, Meta+V
   quickly). Check that the second Meta+V always opens the picker (never
   lost or stuck invisible) and that the delay is acceptable.
2. **Fractional scale on KWin.** At 125 % / 150 % output scaling the picker
   is crisp (no blurry upscaling), correctly sized, placed at the cursor and
   kept on screen; repeat after moving to an output with another scale
   (first Show there may re-render once).
3. **Real-session auto-paste (`org_kde_kwin_fake_input`).** With the package
   in the profile, `dev.bcnelson.spool.paster.desktop` grants the helper fake
   input. Enter in the picker pastes into Konsole (Ctrl+Shift+V), a Qt app
   and a GTK / Firefox field (Ctrl+V); no stray Return or Enter reaches the
   target; `spoolctl status` shows `paste: fake-input`. With a non-US layout
   (e.g. Dvorak) the chord still lands on `v`.
4. **KWallet flows.** First start creates a "Spool clipboard history key"
   item in KWallet without a prompt; restart with the wallet closed:
   `spoolctl status` shows `key: waiting-for-wallet`, the picker shows the
   session items under the unlock panel; opening the wallet merges and
   persists them. `Unlock{KWallet}` from the panel shows KWallet's dialog;
   cancelling reports "dismissed".
5. **FIDO2 hardware test.** `SPOOL_FIDO2_HW_TESTS=1 cargo test -p
   spool-keys --test fido2_hw -- --nocapture --test-threads=1` with one key
   (two touches to enroll, one to unlock; `SPOOL_FIDO2_PIN` /
   `SPOOL_FIDO2_UV=1` for PIN / UV keys). Then the picker's unlock panel
   with a FIDO2 slot: touch, PIN prompt on a `uv` slot, wrong PIN, blocked
   key, timeout, and a PIN typed while the touch request is still waiting.
6. **spool-keyctl plan** (throwaway `--state-dir`, spare security key):
   1. `add fido2` with one key: two touches; `status` shows `fido2-hmac`,
      `uv: no`. With two keys: the device list and choice appear;
      `--device` skips it. On a key with a PIN set: PIN prompt, wrong PIN ->
      "Wrong PIN" and re-prompt.
   2. `add fido2 --uv`: PIN asked at enroll and at every unlock.
   3. `--unlock-slot <fido2 slot> add secret-service`: one touch; with no
      key connected: "No security key connected", exit 4.
   4. With KWallet closed: `add passphrase` shows the KWallet unlock dialog;
      cancelling falls through to the FIDO2 / passphrase slots.
   5. `rotate` with wallet + passphrase + FIDO2 slots: wallet entry replaced
      (KWalletManager shows a new item, old one gone), new passphrase
      prompt, two touches; then `systemctl --user start spool` unlocks and
      picker search works after the index rebuild.
   6. Kill `rotate` (Ctrl-C at the FIDO2 touch, or `kill -9` during
      "Re-encrypting"): `status` shows the pending rotation; starting spoold
      now gives `key: error: interrupted key rotation — run spool-keyctl
      recover` (session-only, nothing touched); `recover` rolls back or
      forward and spoold starts normally afterwards.
   7. While spoold runs, any mutating command exits 3; while a command
      waits at a prompt, `systemctl --user start spool` fails ("state
      directory ... is in use").
   8. `remove` the wallet slot: its KWallet item disappears; `wipe` leaves
      no Spool items in KWallet.
7. **Hyprland plan** (a Hyprland session, never nested from Plasma):
   1. `hyprctl -j cursorpos` / `hyprctl -j activewindow` match the shape of
      `crates/spool-hypr/tests/fixtures/`; `socat -U -
      UNIX-CONNECT:$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket2.sock`
      shows `activewindow>>class,title` then `activewindowv2>>address`.
   2. Run spoold; focus foot, then a browser: `spoolctl status` / logs show
      the app ids, never titles.
   3. With xdg-desktop-portal-hyprland, spoold logs `portal shortcut bound`;
      `hyprctl dispatch global dev.bcnelson.spool.daemon:spool-show` (or the
      bound keys) opens the picker at the cursor.
   4. Pick in foot (expects Ctrl+Shift+V) and in a GTK app (Ctrl+V); repeat
      with `input { kb_layout = us; kb_variant = dvorak }`.
8. **sway hotkey**: `bindsym $mod+v exec spoolctl show`, then the paste
   checks above (virtual keyboard).
9. **GPU picker** (optional): `picker.renderer = "gpu"` on the real GPU;
   check it renders and falls back to software if it cannot (see
   "MemoryDenyWriteExecute" below for llvmpipe / NVIDIA).
10. **External editor**: Ctrl+E on a text item opens the `text/*` editor
    (Konsole + nvim by default) with the text; `:w` adds a new item at the
    top of the picker, a second `:w` updates it, `:q` puts it on the
    clipboard. Ctrl+Shift+E on a Firefox copy offers text and HTML; HTML in
    Kate (`--block`) stores HTML plus plain text. Pasting a GitHub token
    and saving shows "Spool: edit not saved" and keeps the previous version.
    `journalctl --user -u 'spool-edit-*'` shows the editor units;
    `$XDG_RUNTIME_DIR/spool/edit/` is empty afterwards. Check that the
    editor gets focus on Plasma (a newly started window from a background
    service may open behind the active one).

## Picker (`spool-picker`)

A resident Slint UI on a wlr-layer-shell overlay surface, launched once by
spoold and driven over a socketpair on fd 3 (picker protocol v3, INTERFACES.md
section 6).

- Launch: spoold finds `spool-picker` **only** on its own (audited) PATH,
  never next to its binary or from config; the home-manager module puts
  `services.spool.pickerPackage` there. Not found = no picker: `Show` /
  `Pick` answer "not available", the CLI works. Cleared environment;
  restarted if it dies (backoff, at most 5 per minute; a `gpu` picker that
  dies before it is ready comes back with `software`); a Show made while it
  was down is delivered after the relaunch.
- Config (`config.toml`):

  ```toml
  [picker]
  renderer = "software"   # or "gpu"
  prerender = true        # false: free the frame buffers while hidden
  ```

- `spoolctl show` opens it; `spoolctl pick [--raw]` opens it in "return"
  mode: the chosen item is printed (escaped on a TTY) instead of pasted, the
  clipboard is left alone, Esc exits 3.
- Keys: type to search, Up/Down/PgUp/PgDn, Enter = paste, Shift+Enter =
  copy only, Ctrl+Enter = paste as plain text (all act when Enter is
  **released**, so the target window never sees a stray Return), Ctrl+P
  pin, Del delete, Ctrl+E edit in the external editor, Ctrl+Shift+E choose
  which format to edit first (see "Editing items"), Esc close.
- Unlock panel (history locked; the list shows this session's items):
  passphrase or security-key PIN field (Enter unlocks, Tab moves to the
  search box), "Touch your security key" with a spinner and Retry (Ctrl+R),
  the failure reason. Prompts come from `keyslots.json` (passphrase slot ->
  field, FIDO2 slot -> touch, + PIN for `uv` slots or when the key asks).
  The panel disappears when history unlocks by any route. The typed secret
  is moved into a zeroize-on-drop buffer and the Slint property cleared
  after Enter and on every hide; Slint's own internal copies cannot be
  wiped.
- Daemon errors (`BadQuery`, ...) show as a red line in the status bar.
- Renderer: software (default; next frame pre-rendered while hidden, Show
  ~1-2 ms to presentation). `--features gpu` builds (the Nix package) can
  opt in with `SPOOL_PICKER_RENDERER=gpu` / `renderer = "gpu"`: FemtoVG over
  EGL, falling back to software when EGL is missing or the GL is llvmpipe.
  It draws on Show (no pre-render), so it is not faster here: 2026-10-09,
  headless sway gles2 on an RX 6650 XT, release, 50 shows: Show ->
  presented p50 3.0 / p99 5.3 ms (GPU) vs 1.4 / 2.0 ms (software) at scale
  1, 3.2 / 5.3 vs 2.3 / 3.7 ms at scale 2; keystroke latency is the 30 ms
  search debounce either way.
- `MemoryDenyWriteExecute`: the spoold unit sets it and the picker inherits
  it. Mesa llvmpipe JITs and crashes under it (one reason software GL is
  refused); radeonsi (ACO) is fine. Drivers that generate CPU code
  (llvmpipe, possibly NVIDIA's proprietary driver) would need MDWE off for
  the picker. Proposal if that is ever needed: with `renderer = "gpu"`,
  start the picker as its own transient user unit
  (`StartTransientUnit`, `MemoryDenyWriteExecute=no`, the socketpair end as
  `StandardInputFileDescriptor`, dup'ed to fd 3 by a tiny wrapper) instead
  of relaxing spoold.
- Licensing: the picker links Slint under its GPL-3.0-only option, so the
  `spool-picker` package is GPL-3.0-only (Spool's own sources stay MIT); the
  embedded UI font is a Noto Sans subset under OFL-1.1
  (`crates/spool-picker/fonts/OFL.txt`).

## Editing items in an external editor

Ctrl+E in the picker opens the selected item in your editor (its preferred
format: text first); Ctrl+Shift+E first asks which format (text, HTML, an
image, ...). `spoolctl edit` opens the picker to choose the item (there is
deliberately no `spoolctl edit <id>`: the public socket cannot read
history), `spoolctl new [--mime M]` starts from an empty file.

- spoold writes the format to `$XDG_RUNTIME_DIR/spool/edit/<random>/item.<ext>`
  (tmpfs, dir 0700, file 0600) and starts the configured command as a
  transient systemd user unit (`spool-edit-<random>.service`), never as its
  own child (which would inherit its seccomp sandbox).
- Every save (inotify, 300 ms debounce) goes through the same policy as
  `spoolctl copy` (size caps, secret patterns): the first accepted save
  becomes a **new** item (`source_app` `spool.editor`, the original's tags
  but not its pin, `derived_from` = the original); later saves update that
  item in place. The original is never modified. HTML is stored with a
  plain-text version derived from it; an image is stored as whatever type
  its bytes are. A refused save (e.g. it now contains a token) keeps the
  previous version and shows a notification naming only the reason.
- When the editor exits normally, the last accepted version goes on the
  clipboard and the temp directory is deleted; if it crashed, the stored
  version is kept but not put on the clipboard. At most 4 sessions at once;
  works while history is locked.

Config (`config.toml`, or `services.spool.editor.*` in the home-manager
module):

```toml
[editor]
terminal = ["konsole", "--separate", "-e"]
[editor.mime]   # most specific pattern wins; {file} = the temp file
"text/*"    = ["{terminal}", "nvim", "-n", "-i", "NONE", "--cmd", "set noundofile nobackup nowritebackup", "{file}"]
"text/html" = ["kate", "--block", "{file}"]
"image/*"   = ["krita", "--nosplash", "{file}"]
```

The command must **block until you are done**: Spool takes its exit as
"finished". `kate --block`, `code --wait`, `gedit --wait`, a terminal editor
in a terminal that does not hand the window to a running instance
(`konsole --separate -e`, `foot`, `alacritty -e`); Krita and GIMP reuse a
running instance (close it first, or `gimp --new-instance`). An editor that
exits within 2 s without saving triggers a notification saying so. No entry
for a type: "no editor configured for <mime>". The home-manager module
defaults `text/*` to `home.sessionVariables.VISUAL` / `EDITOR` (the unit
never sees a shell-only `$EDITOR`) or `nvim`, in `{terminal}`.

Caveat: the editor runs outside Spool's sandbox and control. It may keep
swap, undo, backup, session or autosave copies of the text elsewhere (Neovim:
`~/.local/state/nvim/{swap,undo,shada}` unless started as above; Kate:
swap files and sessions; Krita: autosave). Spool deletes only its own temp
file.

## History encryption and the state directory

`$XDG_STATE_HOME/spool` (0700; `SPOOL_STATE_DIR` overrides) holds the
SQLCipher `history.db` (+ WAL), `blobs/` (encrypted files for items over
1 MiB), `history.db.kcv`, `keyslots.json`, the encrypted search `index/` and
the `state.lock` that keeps spoold and spool-keyctl apart. spoold captures
into an in-memory session store from the first second; with `key_provider =
"secret-service"` (default) it creates or unlocks a key slot in the default
Secret Service collection (KWallet / gnome-keyring) without ever prompting,
then merges the session into the encrypted database. While the wallet is
locked, `spoolctl status` shows `key: waiting-for-wallet` and history is
memory-only until it opens (or the picker's unlock panel opens it with a
passphrase / FIDO2 slot). Fatal key errors (`key: error: ...`, including an
unfinished `spool-keyctl rotate`) keep the session store; nothing is ever
wiped automatically. `key_provider = "session"` never persists anything.

A plaintext `history.db` from an M1 development build is renamed to
`history.db.m1-plaintext.bak` (plus `-wal`/`-shm`) with a warning: delete it.
`SPOOL_DEV_PLAINTEXT=1` keeps using a plaintext database (development only;
logged loudly).

Auto-paste: spoold is non-dumpable (it holds the data key), so KWin cannot
match it to a desktop file; pasting runs in the secret-free `spool-paster`
helper, which `dev.bcnelson.spool.paster.desktop` grants
`org_kde_kwin_fake_input`. Settings: `auto_paste = true` (default) and
`paste_terminals = [...]` (app ids that paste with Ctrl+Shift+V; replaces
the built-in list). `spoolctl status` shows the desktop integration in use.

## Key management (`spool-keyctl`)

Key slots are managed by a separate, interactive tool, never over spoold's
public socket (any process of the same user can reach that). It only runs
while spoold is **stopped**: it takes spoold's single-instance lock
(`<socket>.lock`), the state directory's `state.lock` and spool-search's
`index.lock`, all non-blocking, and holds them until it exits, so it refuses
(exit 3) while spoold runs and a spoold started meanwhile refuses to start,
whatever socket path either uses. `status` is read-only and also works
while spoold runs.

```sh
systemctl --user stop spool
spool-keyctl status                 # alias: list
spool-keyctl add passphrase         # min. 12 characters (--allow-short overrides, with a warning)
spool-keyctl add fido2 [--device /dev/hidrawN] [--uv]   # asks which key if several; two touches
spool-keyctl add secret-service     # (re-)add a KWallet / gnome-keyring slot
spool-keyctl remove <slot-id> [--force]                 # --force (+ typing "remove") for the last slot
spool-keyctl rotate [--allow-short] # new data key, every slot re-enrolled, history re-encrypted
spool-keyctl recover                # finish or undo an interrupted rotate
spool-keyctl wipe                   # type "wipe": destroy all secrets, delete history + index + slots
systemctl --user start spool
```

Global options: `--state-dir DIR` (or `SPOOL_STATE_DIR`, same resolution as
spoold), `--unlock-slot SLOT_ID` (unlock only with that slot).

- **Unlock first.** `add`, `remove`, `rotate` and `recover` first unlock the
  data key with an existing slot: Secret Service slots (the wallet may show
  its unlock dialog), then FIDO2 (touch; PIN prompt if the key asks), then
  passphrase (one prompt tried against every passphrase slot, 3 attempts).
  A candidate key is only accepted if it opens `history.db`. Session and
  tpm2 slots are skipped. `wipe` needs no unlock (it is also the way out
  after losing every unlock method).
- **Prompts** use `/dev/tty` with echo off (Ctrl-C/Ctrl-D abort and restore
  the terminal); secrets live in zeroize-on-drop buffers and are never
  printed. Results go to stdout, progress to stderr. Non-dumpable, umask 077.
- **Exit codes:** 0 ok, 1 error, 2 usage, 3 spoold is running, 4 no slot
  could be unlocked, 5 done but some provider secrets could not be destroyed
  (listed; e.g. wallet locked), 6 an interrupted rotation needs `recover`,
  7 aborted / not confirmed.

### Rotation and crash safety

spool-keys' `KeySlots::rotate` swaps `keyslots.json` before the database is
re-keyed, so a crash in between would leave a database that only the old,
already destroyed key opens. `spool-keyctl rotate` therefore builds the new
slot set beside the old one and destroys nothing until the history is
re-keyed:

1. Unlock the old key K0 (validated on the DB).
2. `keyslots.json.next` = `KeySlots::create_new` (random K1, new data key
   id) with an in-memory placeholder slot, then one freshly enrolled slot
   per old slot (new wallet item; new passphrase, may equal the old one; new
   FIDO2 credential on the same key, same `uv`), then the placeholder is
   removed. A failed enroll offers retry / skip (drop that slot) / abort
   (rolls back: new secrets destroyed, `.next` deleted).
3. `Store::rekey(K1)` (itself crash-safe: after any crash exactly one of
   K0/K1 opens the DB and `open_encrypted` completes or undoes it).
4. `keyslots.json` is copied to `keyslots.json.old`, then `.next` is renamed
   over `keyslots.json` (atomic, directory fsynced).
5. The `.old` slots' provider secrets are destroyed, `.old` is deleted, and
   `index/` (encrypted with K0) is deleted; spoold rebuilds it at the next
   unlock.

After a crash: if `.next` exists, `recover` unlocks a slot from either file,
sees which key opens the DB and rolls back (K0) or forward (K1, steps 4-5);
a stray `.old` next to `.next` is just a copy and is removed without
touching its secrets. If only `.old` exists, `recover` (or the next `add` /
`remove` / `rotate`) runs step 5. Until then `status` reports the pending
state, mutating commands exit 6, and spoold neither creates nor unlocks any
slot: it stays session-only with `key: error: interrupted key rotation —
run spool-keyctl recover` and offers no unlock prompts. Unit tests inject a
crash after each step (`CrashPoint`) and check that some slot still opens
the history and that `recover` ends with wallet items == slots and all items
intact.

`wipe` destroys provider secrets first (every slot file, then
`destroy_all` for orphaned Spool wallet items), then deletes `history.db`
(+WAL/SHM/kcv), `blobs/`, the index, the slot files and any M1 plaintext
backup. On copy-on-write filesystems, snapshots and backups old files can
survive; they stay encrypted and are unreadable once the slot secrets are
gone (passphrase / FIDO2 slots have no external secret: an old
`keyslots.json` copy plus the passphrase or key still unlocks an old DB
copy, so delete such snapshots).

## Other compositors

spoold gets the same events and capabilities on every compositor (matrix in
INTERFACES.md section 12): KWin uses the KWin script (Meta+V, focus, cursor)
and fake input; Hyprland uses the GlobalShortcuts portal
(xdg-desktop-portal-hyprland, `LOGO+v`), `spool-hypr` IPC for focus and
cursor, and the virtual keyboard; sway / other wlroots compositors use a
user binding (`bindsym $mod+v exec spoolctl show`; xdg-desktop-portal-wlr has
no GlobalShortcuts), wlr-foreign-toplevel for focus, no cursor (centred
picker), and the virtual keyboard.

The portal needs an app id for a host process: `spool-portal` calls
`org.freedesktop.host.portal.Registry.Register("dev.bcnelson.spool.daemon")`
first on its connection, and xdg-desktop-portal (1.22) accepts that only if
`dev.bcnelson.spool.daemon.desktop` is findable in
`$XDG_DATA_DIRS/applications` with an existing `Exec` binary, i.e. the
package must be in the user's profile. Otherwise binding fails with `An app
id is required`.

Hyprland could not be tested in isolation: 0.56 refuses to nest in headless
sway (needs `xdg_wm_base` v6, sway offers v5) and otherwise falls back to the
DRM backend, which tries to take over the real seat through logind. Never
start it from a desktop session; use the manual plan above.

## Benchmarks

Criterion benches for `spool-core` (ingest policy and store); not built by
`cargo test`, so `nix build .#spool` is unaffected.

```sh
cargo bench -p spool-core                  # everything
cargo bench -p spool-core --bench policy   # detect_secret / evaluate / plan
cargo bench -p spool-core --bench store    # insert / recent / latest / retention
cargo bench -p spool-core -- 'plan/'       # filter by id
```

The store bench caches its 5k/50k-item fixtures under
`<target>/release/spool-bench-fixtures/` (delete after changing the schema
or the generator).

Baseline, 2026-10-08, sierra (shared KDE desktop), medians, measured under
the agent `lowprio.slice` (CPUWeight=20, IOWeight=10) with other builds
running, so treat absolute numbers as indicative; file-backed inserts are
fsync-bound and very noisy there.

| Bench | 1 KiB | 64 KiB | 1 MiB | 8 MiB |
| --- | --- | --- | --- | --- |
| `detect_secret/prose_code` | 0.50 µs | 107 µs | 1.78 ms | 14.8 ms (541 MiB/s) |
| `detect_secret/unicode_prose` | 0.29 µs | 15 µs | 0.24 ms | 2.0 ms (4.0 GiB/s) |
| `detect_secret/secret_at_end` | 0.50 µs | 107 µs | 1.79 ms | 16.5 ms (486 MiB/s) |
| `detect_secret/sk_runs` (worst) | 6.0 µs | 376 µs | 6.08 ms | 48.6 ms (165 MiB/s) |
| `detect_secret/sk_long_body` | 6.1 µs | 387 µs | 6.24 ms | 49.7 ms (161 MiB/s) |
| `detect_secret/eyJ_run` | 3.2 µs | 195 µs | 3.13 ms | 25.0 ms (319 MiB/s) |
| `detect_secret/jwt_partial` | 1.7 µs | 96 µs | 1.56 ms | 12.4 ms (643 MiB/s) |
| `detect_secret/pem_partial` | 1.8 µs | 122 µs | 1.76 ms | 15.0 ms (534 MiB/s) |
| `detect_secret/token_prefixes` | 1.7 µs | 106 µs | 1.69 ms | 14.0 ms (573 MiB/s) |
| `detect_secret/digits` | 0.32 µs | 15 µs | 0.23 ms | 3.2 ms (2.5 GiB/s) |
| `detect_secret/base64ish` | 0.32 µs | 14 µs | 0.25 ms | 3.4 ms (2.3 GiB/s) |
| `evaluate/text_prose_code` | 2.7 µs | 157 µs | 2.60 ms | 19.9 ms (402 MiB/s) |
| `evaluate/text_unicode` | 3.9 µs | 227 µs | 3.88 ms | 31.0 ms (258 MiB/s) |
| `evaluate/text_secret_at_end` | 1.3 µs | 136 µs | 2.33 ms | 18.3 ms (437 MiB/s) |
| `evaluate/png` (no scan) | 1.1 µs | 23 µs | 0.20 ms | 1.7 ms (4.7 GiB/s) |

`plan`: Firefox offer (10 mimes) 258 ns, KDE/Qt offer (12) 304 ns, hostile
256-mime offer 5.5 µs (9.0 µs with an `image/*` allowlist entry).

Store benches after the index/`change_seq` fix (migration 3), 2026-10-09,
sierra, medians, same caveats. Previous baseline in parentheses.

| Store bench | file 5k | file 50k | memory 5k | memory 50k |
| --- | --- | --- | --- | --- |
| `store_insert_new` | 26 µs (1.42 ms) | 131 µs (16.5 ms) | 16.7 µs (2.28 ms) | 16.8 µs (13.3 ms) |
| `store_insert_duplicate` (bump) | 0.68 ms (2.03 ms) | 2.9 ms (24.5 ms) | 9.2 µs (2.79 ms) | 9.1 µs (15.2 ms) |
| `store_recent_50` | 149 µs (3.68 ms) | 161 µs (36.8 ms) | 323 µs (3.62 ms) | 164 µs (35.7 ms) |
| `store_latest` | 13.9 µs (1.60 ms) | 15.3 µs (17.4 ms) | 9.8 µs (1.59 ms) | 9.9 µs (16.5 ms) |
| `store_retention_sweep_10pct` | 27 ms (444 ms) | 68 ms (21.9 s) | 5.1 ms (172 ms) | 49 ms (—) |

Insert, `latest` and `recent` are index seeks (flat in the item count); the
sweep is linear in the number deleted.
