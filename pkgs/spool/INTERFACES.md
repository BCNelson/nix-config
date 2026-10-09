# Spool interfaces

The contract between Spool's crates and processes, as built (M1-M7
integrated). README.md covers building, testing and day-to-day use; this
file is the reference for anyone changing a crate boundary, a wire format
or a launch contract.

## Contents

1. Rules
2. Crate map
3. Processes, launch contracts and locks
4. State directory
5. Public socket (`spool-proto`)
6. Picker channel (protocol v2) and `spool-picker`
7. Paste helper channel (`spool-paster`)
8. Library crates: `spool-core`, `spool-crypto`, `spool-keys`,
   `spool-search`, `spool-wayland`, `spool-kwin`, `spool-paste`,
   `spool-compositor`, `spool-portal`, `spool-hypr`
9. `spoold` internals
10. `spoolctl` and `spool-keyctl`
11. Security model
12. Compositor matrix
13. Testing

## 1. Rules

1. **Public signatures are stable.** Changing a `pub` item listed here means
   updating every user in the workspace and this file in the same change.
   Postcard enums and structs are positional: new variants / fields are
   appended, never inserted.
2. **Dependencies** come from the root `[workspace.dependencies]`
   (`foo = { workspace = true }`).
3. **Never log clipboard content, secrets, queries or window titles.** Allowed
   in logs: `item::hash_prefix`, byte lengths, mimes, app ids, decision
   reasons, slot ids, provider kinds.
4. `unsafe` is forbidden (`#![forbid(unsafe_code)]`) in `spool-proto`,
   `spool-core`, `spool-wayland`, `spool-keys`, `spool-kwin`, `spool-paste`,
   `spool-compositor`, `spool-portal`, `spool-hypr`, `spool-keyctl`.
   `spool-crypto`, `spool-search` and `spool-picker` are
   `#![deny(unsafe_code)]` with local exceptions: `spool-crypto::mlock`, one
   `StableDeref` impl in `spool-search::directory`, the picker's fd adoption
   (`main.rs`) and EGL glue (`gpu.rs`). `spoold` uses it only in `security`
   (libc: prctl, umask, `pre_exec` dup2) and tests.
5. `cargo test --workspace` (what `nix build .#spool` runs, minus the
   picker) must pass in the Nix sandbox: no Wayland, no session bus, no
   network, no `$HOME`. Everything that needs a compositor, a bus, a keyring
   or hardware is gated on an environment variable and returns early
   otherwise (section 13).
6. Tests never touch the user's session: private `XDG_RUNTIME_DIR`, private
   `dbus-daemon`, headless sway or nested virtual KWin, throwaway
   gnome-keyring, temp state directories. Hyprland is never run.

## 2. Crate map

Dependency direction is top to bottom; only `spoold` pulls everything
together. The picker depends on `spool-proto` only.

| Crate | Kind | Owns |
| --- | --- | --- |
| `spool-proto` | lib | wire types and framing: public socket (`PublicReq`/`PublicResp`), picker channel v2 (`PickerReq`/`PickerEvt`), `Hello`, socket path |
| `spool-crypto` | lib | `DataKey` / `SubKey` (HKDF labels), chunked AEAD file format, small sealed blobs, best-effort `mlock` |
| `spool-core` | lib | item model, two-phase ingest `policy`, SQLite/SQLCipher `store` (session / encrypted / plain, blobs, merge, rekey, search feed), TOML `config` |
| `spool-keys` | lib | `keyslots.json` (LUKS-style slots), providers: Secret Service (oo7), session, passphrase (Argon2id), FIDO2 `hmac-secret` (libfido2, feature `fido2`, default on) |
| `spool-search` | lib | Tantivy index, encrypted directory (`Label::Index`), typing-mode / explicit queries, ranking |
| `spool-wayland` | lib | `ext-data-control-v1` clipboard watcher/owner on a dedicated thread |
| `spool-kwin` | lib | KWin script loader, `dev.bcnelson.spool.Kwin` D-Bus service, `ActiveWindowTracker` |
| `spool-paste` | lib + bin `spool-paster` | paste chord via `org_kde_kwin_fake_input` or `zwp_virtual_keyboard_v1`, keycode resolution, terminal list, the helper process, `ToplevelTracker` (wlr-foreign-toplevel) |
| `spool-compositor` | lib | compositor-neutral `CompositorEvent`, `EventSink`, `CompositorKind`, `CursorProvider`, `Capabilities` |
| `spool-portal` | lib | hotkey via the XDG GlobalShortcuts portal (ashpd) |
| `spool-hypr` | lib | Hyprland IPC: focus from `.socket2.sock`, cursor from `j/cursorpos` |
| `spoold` | bin | daemon: orchestrator, socket server, key flow, picker host, desktop integration, hardening |
| `spoolctl` | bin | CLI client |
| `spool-keyctl` | bin + lib | offline, interactive key-slot management |
| `spool-picker` | bin (GPL-3.0-only via Slint) | resident Slint layer-shell picker; separate Nix package |

Feature flags: `spool-proto/tokio` (async framing), `spool-keys/fido2`
(default), `spoold/test-hooks` (debug sockets; tests only, never packaged),
`spool-picker/gpu` (FemtoVG/EGL; on in the Nix package), `spool-picker/timing`
and `spool-picker/dev` (instrumentation and the `mock_daemon` example;
never shipped).

## 3. Processes, launch contracts and locks

| Process | Started by | Talks to | Holds secrets |
| --- | --- | --- | --- |
| `spoold` | systemd user unit (`systemd-service.nix`, `WantedBy=graphical-session.target`) | Wayland, session bus, Secret Service, its socket, picker, paster | data key, history |
| `spool-picker` | spoold, once, resident | spoold over fd 3; Wayland (layer shell) | typed passphrase / PIN briefly (zeroize-on-drop) |
| `spool-paster` | spoold (only with `auto_paste` and a focus source) | spoold over its stdin socketpair; Wayland | none |
| `spoolctl` | user | spoold's public socket | none |
| `spool-keyctl` | user, while spoold is stopped | state directory, Secret Service, FIDO2 keys, `/dev/tty` | data key during an operation |

**spoold startup order** (security relevant): umask 077 and
`PR_SET_DUMPABLE=0` -> logging (journald, or stderr with `--log-stderr`) ->
PATH audit (`SPOOL_INSECURE_PATH_OK=1` overrides, development only) ->
config (`--config` / `SPOOL_CONFIG` / `$XDG_CONFIG_HOME/spool/config.toml`)
-> **public socket bind** (single-instance guard, `<socket>.lock`) -> state
directory (0700) and **`<state>/state.lock`** -> plaintext-M1-DB quarantine
-> in-memory session store (capture starts now) -> desktop integration ->
Wayland thread -> orchestrator -> key flow (`key_provider =
"secret-service"`) -> resident picker (if `spool-picker` is on the audited
PATH) -> socket server. SIGTERM/SIGINT -> bounded shutdown (section 9).

**Picker launch**: spoold looks for `spool-picker` only via
`security::find_on_audited_path` (PATH dirs that pass the audit; file under
`/nix/store` or root-owned and not group/world-writable), never next to its
own binary or from config. Socketpair end on fd 3 (CLOEXEC cleared),
environment cleared except `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`,
`XDG_CONFIG_HOME`, `HOME`, `LANG`, `LC_*`, `SPOOL_PICKER_LOG`, plus
`SPOOL_PICKER_FD=3`, `SPOOL_PICKER_RENDERER=software|gpu`,
`SPOOL_PICKER_PRERENDER=1|0`. It inherits spoold's sandbox (incl.
`MemoryDenyWriteExecute`). Not found = no picker: `Show`/`Pick` answer
`NotYetImplemented`.

**Paster launch**: `RemotePaster::locate(current_exe)` (sibling of spoold,
else PATH) -> `RemotePaster::spawn`: environment cleared except
`XDG_RUNTIME_DIR` / `WAYLAND_DISPLAY`, a socketpair as stdin, restarted once
if it dies. The package installs `dev.bcnelson.spool.paster.desktop`
(`X-KDE-Wayland-Interfaces=org_kde_kwin_fake_input`, `Exec=<out>/bin/spool-paster`)
so KWin grants it fake input; `dev.bcnelson.spool.daemon.desktop` (portal
app id, no interface grant) names spoold.

**Locks** (all non-blocking exclusive `flock`s; lock files are never
deleted):

| Lock | spoold | spool-keyctl (mutating commands) | spool-keyctl `status` |
| --- | --- | --- | --- |
| `<socket>.lock` (`$XDG_RUNTIME_DIR/spool/sock.lock` or `$SPOOL_SOCKET.lock`) | whole lifetime (`ipc::bind`) | held until exit | probed |
| `<state>/state.lock` | whole lifetime (`paths::lock_state_dir`) | held until exit | probed |
| `<state>/index.lock` | while the on-disk index is open (spool-search writer lock) | held until exit | probed |

So spool-keyctl refuses (exit 3) while spoold runs, and spoold refuses to
start (non-zero exit, "state directory ... is in use") while spool-keyctl
works, whatever socket path either was started with.

## 4. State directory

`$XDG_STATE_HOME/spool` (else `~/.local/state/spool`; `SPOOL_STATE_DIR` /
`--state-dir` override; 0700, fixed if looser):

| Path | Content |
| --- | --- |
| `history.db` (+`-wal`, `-shm`) | SQLCipher database, key = `Label::Db` sub-key |
| `history.db.kcv` | key check value of the current key (+ rekey state) |
| `blobs/<32 hex>.bin` | representations > 1 MiB, chunked AEAD (`Label::Blob`) |
| `keyslots.json` | key slots (0600), see `spool-keys` |
| `keyslots.json.next` / `.old` | an unfinished `spool-keyctl rotate`; spoold then stays session-only with key state `error: interrupted key rotation — run spool-keyctl recover` and offers no unlock prompts |
| `index/`, `index.rebuild/`, `index.lock` | search index (`Label::Index`), its rebuild staging dir and writer lock |
| `state.lock` | spoold / spool-keyctl exclusion |
| `history.db.m1-plaintext.bak*` | a quarantined M1 development database (delete it) |

## 5. Public socket (`spool-proto`)

- `paths::public_socket_path()` = `$SPOOL_SOCKET` or
  `$XDG_RUNTIME_DIR/spool/sock` (dir 0700, refused if looser; socket 0600).
- Framing: `u32` BE length + postcard payload, `MAX_FRAME = 16 MiB`.
  `write_frame` / `read_frame` (sync), `write_frame_async` /
  `read_frame_async` (feature `tokio`, not cancel-safe), `encode_frame`,
  `decode_payload`. `FrameError { Eof, TooLarge(usize), Io, Codec }`; `Eof` =
  clean close on a frame boundary.
- Handshake: `Hello { proto: u16 }`, `PROTO_VERSION = 1`; client first,
  server answers with its own and closes on mismatch.
- `PublicReq { Show, Pick, Copy{mime, data}, Current, Pause{secs: Option<u32>}, Resume, Status }`.
- `PublicResp { Ok, Current{mime, data}, Empty, Status(StatusInfo), Error{code: ErrorCode, message}, NotYetImplemented, Picked{mime, data}, Cancelled }`
  (`NotYetImplemented` = no picker on this install).
- `ErrorCode { BadRequest, TooLarge, RateLimited, Forbidden, Unavailable, Locked, Internal }`.
- `StatusInfo { version, paused: PauseState, item_count, compositor:
  Option<String>, primary_enabled, encrypted, unlocked, key_state: String,
  capabilities: Option<Capabilities> }`. `key_state`: `locked |
  waiting-for-wallet | unlocking | ready | session-only | dev-plaintext |
  error: <reason>`. `PauseState { Recording, PausedUntil{unix_ms},
  PausedIndefinitely }`.
- `Capabilities { compositor: kwin|hyprland|sway|other, hotkey:
  kwin-script|portal|external, focus: kwin-script|hyprland-ipc|wlr-foreign-toplevel|none,
  cursor: bool, auto_paste: bool, paste_backend: fake-input|virtual-keyboard|none }`.
- Server rules (`spoold::ipc`): peer must pass `security::check_peer` (same
  uid, known pid, readable non-Flatpak `/proc/<pid>/root`; fail closed);
  `Show`/`Pick` rate-limited (`RATE_BURST = 5` per `RATE_WINDOW = 2 s`,
  shared with the hotkey path); `IDLE_TIMEOUT = 30 s`; a `Pick` reply has no
  time limit, a client that closes or speaks out of turn while waiting drops
  it.
- Semantics: `Copy` -> `manual_item` (size caps, secret patterns on text) ->
  publish + store (paused: publish only; a secret is published but never
  stored and never kept alive). `Show` targets the tracker's current window;
  `Pick` opens the picker in return mode (section 6).

## 6. Picker channel (protocol v2) and `spool-picker`

Types in `spool_proto::picker`; served by `spoold::picker::ResidentPicker`
with `spoold::unlock` for the unlock panel.

- Handshake: the picker sends `Hello { proto: PICKER_PROTO_VERSION (2) }`
  first; spoold answers `Hello::picker()` and closes after it on a mismatch
  (`is_picker_compatible`). EOF = the picker exits 0. Requests are read into
  zeroize-on-drop buffers (they may carry a passphrase).
- `PickerReq`: `Ready` (once; first page pre-rendered), `Query{seq, q,
  filters: QueryFilters, offset, limit}`, `Thumb{seq, id, mime}`,
  `Select{id, mode}`, `Pin{id, on}`, `Delete{id}`, `Tag{id, tag, on}`,
  `Unlock{provider, secret: Option<UnlockSecret>}`, `Hidden{reason}`.
- `PickerEvt`: `Show{cursor, output, target_window, scale_hint: Option<f64>}`,
  `Hide`, `Page{seq, offset, items, more}`, `Thumb{seq, id, bytes}`,
  `Error{seq: Option<u32>, code: PickerErrorCode, message}`,
  `Locked{providers: Vec<UnlockPrompt>}`, `Unlocked`, `UnlockFailed{provider,
  reason}`, `IndexProgress{done, total}`, `NewItem{preview}`. `PartialEq`,
  not `Eq`.
- `seq`: one picker counter for `Query` and `Thumb`; every request gets
  exactly one answer with its `seq` (page/thumb or `Error`), in any order;
  the picker keeps only the latest query's page. `WireItemId = i64`.
- `HideReason { Esc, FocusLost, Selected, Requested, Closed }`: every shown
  -> hidden transition is reported (also after `Hide`); `Selected` precedes
  its `Select`.
- `SelectMode { Copy, Paste, PastePlain }`: Enter = `Paste`, Shift+Enter =
  `Copy`, Ctrl+Enter = `PastePlain`, click = `Paste`. The picker acts on the
  Enter key's **release** (mode taken from the modifiers at press), so the
  window that gets focus back never receives a stray Return; a pending Enter
  is dropped on focus loss or hide.
- `PickerErrorCode { BadQuery, NotFound, Locked, Unavailable, Internal }`;
  messages never contain the query or item content.
- Unlock types: `UnlockProvider { KWallet, Passphrase, Fido2 }`,
  `UnlockPrompt { Passphrase, Fido2Touch, Fido2Pin }`, `UnlockFailReason {
  WrongSecret, PinInvalid, PinBlocked, Dismissed, Unavailable, Other }`,
  `UnlockSecret` (zeroized on drop, redacted `Debug`, wire-identical to a
  postcard `String`; `expose()`, `into_inner()`). Also `QueryFilters`,
  `CursorPos`, `PreviewKind`, `ItemPreview`.

What spoold does:

- `Query` -> `Request::Search{limit: min(limit, 499) + 1}` -> `Page{more}`
  or `Error{BadQuery | Internal("search failed") | Unavailable}`. While
  locked, the session store answers. Requests are forwarded in arrival
  order; answers come from their own tasks.
- `Thumb` -> `Request::Thumb` (png/jpeg/webp only, <= 8 MiB; `NotFound` if
  gone). The picker never receives an item's full text.
- `Pin`/`Delete`/`Tag` -> `Request::Edit` (tags 1..=64 chars, no control
  chars or `/`).
- `Locked{prompts}` right after the handshake whenever history is locked and
  whenever the prompt set changes (`unlock::prompts(dir, need_pin)`:
  `Passphrase` per passphrase slot, `Fido2Touch` per FIDO2 slot, `Fido2Pin`
  for `uv` slots or after `NeedsSecret`; empty while waiting for KWallet or
  while a key rotation is unfinished). `Unlocked` when history opens by any
  route.
- `Unlock`: one attempt per provider at a time (`unlock::attempt`, through
  the shared `OpenGate`); a duplicate touch request is ignored, a PIN that
  arrives during a running FIDO2 attempt is queued and tried when it ends;
  `Unlock{KWallet}` retries the Secret Service slot with a **prompting**
  provider (the user asked). Error mapping: `WrongKey` -> `WrongSecret`;
  `PinInvalid`, `PinBlocked`, `Dismissed` -> themselves; `Unavailable` /
  `Locked` / `NoProvider` -> `Unavailable`; `NeedsSecret(Fido2Hmac)` -> no
  failure, `Locked{.., Fido2Pin}`; `NoSlotUnlocked` -> the most specific
  attempt's reason; anything else -> `Other`. `Opened` goes to the
  orchestrator as `KeyEvent::Opened` (same merge + switch as the key flow).
  An unfinished rotation -> `Unavailable`, nothing tried.
- `Show` while the picker is down (crashed, backing off) is delivered after
  the next handshake if it is < 5 s old. Restart backoff 250 ms doubling to
  30 s (reset after 60 s up), at most 5 restarts per 60 s; an incompatible
  `Hello` stops restarting (`Show` -> `Unavailable`). A `gpu` picker that
  dies before `Ready` comes back with `software`. A visible picker that
  dies counts as `Hidden{Closed}`.
- `Pick` (public socket): cancels an older pick, shows with no target; the
  next `Select` returns the item's preferred representation (text first;
  `PastePlain`: text only) as `Picked` (touch, no publish); `Hidden` (not
  `Selected`), a new `Show`/`Pick` or shutdown -> `Cancelled`.

`spool-picker` itself: wlr-layer-shell overlay surface (keyboard
interactivity exclusive while shown), software renderer by default (next
frame pre-rendered while hidden), FemtoVG/EGL with `SPOOL_PICKER_RENDERER=gpu`
in `gpu` builds (falls back to software when EGL fails or the GL is
llvmpipe/softpipe; `SPOOL_PICKER_GPU_ALLOW_SOFTWARE=1` overrides, tests
only). Non-dumpable; logs only to stderr (`SPOOL_PICKER_LOG` filter), never
item content or secrets. Theme from `kdeglobals`; embedded Noto Sans subset
(OFL-1.1). Fractional scale via `wp_fractional_scale_v1`; `scale_hint` from
spoold pre-renders at the right scale.

## 7. Paste helper channel (`spool-paster`)

Stream socketpair (the helper's stdin), big endian:

- helper -> parent once: `b"SPL1"`, then `0, backend` (0 = fake input, 1 =
  virtual keyboard) or an error (`kind, u16 len, message`).
- parent -> helper: `chord` (0 = Ctrl+V, 1 = Ctrl+Shift+V, 2 =
  Shift+Insert), `has_layout` (0/1), `layout: u32`.
- helper -> parent: `0` (ok) or an error. Error kinds: 1 Unsupported,
  2 Connect, 3 Wayland, 4 ThreadGone. EOF ends the helper.

Only chords cross this channel, never content. Any same-user process can
run `spool-paster` (the desktop-file grant is per executable) but can only
press paste chords with it.

## 8. Library crates

### spool-core

- `item`: `ItemId(pub i64)`, `Selection { Clipboard, Primary }` (`as_str`,
  `parse`), `ItemFlags { PINNED, SENSITIVE }`, `Representation { mime,
  alias_of: Option<String>, data: Bytes }` (alias reps have empty data),
  `NewItem { selection, source_app, created_at, flags, hash, preview, reps }`,
  `Item { id, change_seq, created_at, last_used_at, selection, source_app,
  flags, hash, preview, total_size, reps }` (`resolve(mime)`, `mimes`,
  `preferred`), `ItemSummary`, `ItemHash = [u8; 32]`, `dedupe_hash(key,
  canonical_rep)` (keyed BLAKE3 of `mime\0data`), `hash_prefix`, `TEXT_MIMES`.
- `config`: `Config { max_age_days = 14, max_items = 5000, primary_selection
  = false, excluded_apps, max_rep_bytes = 8 MiB, max_item_bytes = 16 MiB,
  fetch_timeout_ms = 2000, mime_allowlist, key_provider: KeyProviderSetting
  { SecretService (default), Session }, auto_paste = true, paste_terminals:
  Option<Vec<String>>, picker: PickerSettings { renderer: PickerRenderer {
  Software (default), Gpu }, prerender = true } }`, `deny_unknown_fields`
  everywhere. `load`, `from_toml_str`, `default_path`, `retention()`,
  `fetch_timeout()`. `DEFAULT_MIME_ALLOWLIST`: text variants,
  `text/uri-list`, `text/html` (stored, never rendered), `image/png`,
  `image/jpeg`, `image/webp`; not SVG, GIF or file-manager cut/copy lists.
- `policy` (two phases: plan what to fetch from the offered mimes, then
  evaluate the fetched data):
  - `OfferInfo { selection, mimes, source_app, at }`; `FetchPlan { Skip(SkipReason),
    CheckHintFirst{timeout}, Fetch(FetchSpec) }`; `FetchSpec { mimes, aliases,
    per_rep_cap, total_cap, timeout }`; `SkipReason { Paused, PrimaryDisabled,
    ExcludedApp, NothingAllowlisted }`.
  - `Decision { Store(NewItem), Drop(DropReason), PurgePrevious{previous} }`;
    `DropReason { Paused, PasswordManagerHint, Secret(SecretKind), Empty,
    TooLarge, NoUsableData }`; `SecretKind { PemPrivateKey, GithubToken,
    AwsAccessKey, OpenAiStyleKey, Jwt, SlackToken, AgeSecretKey, OtpCode }`.
  - `Policy::new(Config, hash_key)`, `plan`, `plan_after_hint`,
    `hint_is_secret`, `evaluate`, `manual_item` (caps + hashing, allowlist
    not applied, secret patterns applied to text), `detect_secret`.
  - `PolicyState`: pause/resume, `record_stored/dropped/purged`,
    `keepalive_candidate`, `clear_candidate` (clear within `CLEAR_WINDOW =
    60 s` purges the previous item), `remap_ids` (after the session merge).
  - Text collapsing: one canonical text mime fetched, the rest aliased.
- `store`:
  - Constructors: `Store::open(path, Option<&[u8; 32]>)` (plain;
    development, `SPOOL_DEV_PLAINTEXT=1`), `open_in_memory()` (plain),
    `open_encrypted(dir, &DataKey)` / `open_encrypted_with(.., &StoreOptions
    { cipher_memory_security })`, `open_session(&DataKey)` (in-memory,
    pre-unlock). `kind() -> StoreKind { Plain, Encrypted, Session }`.
  - Errors: `WrongKey`, `NotEncrypted`, `Corrupt`, `Blob { item, file,
    source }` (missing/tampered blob: `get`/`latest` fail, `recent`/`delete`
    work), `Unsupported`.
  - `insert -> InsertOutcome { Inserted(id), Bumped(id) }` (bump = same hash
    as the newest item of that selection), `get`, `latest`, `recent(limit)`
    (never reads blobs), `touch`, `set_pinned`, `set_tag`, `delete`
    (tombstone), `retention_sweep(now, RetentionLimits)` (pinned exempt),
    `count`, `hash_key()`.
  - Blobs: reps > `INLINE_MAX` (1 MiB) in encrypted stores ->
    `blobs/<hex>.bin` (`Label::Blob`), written before the DB commit, removed
    after commit + checkpoint; `gc_orphan_blobs` at open.
  - `merge_from_session(&Store) -> MergeReport { inserted, bumped,
    blob_files, ids }` (one transaction, oldest first, re-hashed).
  - `rekey(&DataKey)`: crash-safe (`meta.rekey_in_progress` + `rekey_blobs`);
    after any crash exactly one of old/new key opens the store and
    `open_encrypted` completes or undoes it. Recompile `Policy` afterwards.
    Only `spool-keyctl rotate` rekeys (offline).
  - `Store::wipe(dir)`.
  - Search feed: `db_uuid`, `max_change_seq`, `items_for_index(after, upto,
    limit, max_text) -> Vec<IndexItem>`, `tombstones_after`, `summaries(ids)
    -> Vec<TaggedSummary>`, `recent_page(offset, limit)`.
  - Constants: `DB_FILE_NAME`, `BLOB_DIR_NAME`, `KCV_FILE_NAME`,
    `STATE_LOCK_FILE_NAME`. Timestamps are ms since epoch; `change_seq`
    comes from the `meta` counter. `MIGRATIONS` is append-only.

### spool-crypto

Key types have no `Clone` (explicit `try_clone()`), redacted `Debug`,
zeroize on drop, best-effort `mlock`.

- `DataKey`: `generate()`, `from_bytes(Zeroizing<[u8; 32]>)`, `expose()`,
  `try_clone()`, `derive(Label) -> SubKey`. `Label { Db, Index, Blob, Hash }`
  = HKDF-SHA256 `info` `spool/{db,index,blob,hash}/v1` (vectors pinned).
- `SubKey`: `label()`, `expose()`, `try_clone()`, `sqlcipher_pragma_hex()`.
- Chunked files: header `"SPLCRY01" | ver 1 | chunk_log2 (12..=20, default
  16) | 0u16 | salt[32]` (44 bytes), then `ct || tag16` per chunk; file key =
  HKDF(sub, salt, `spool/file/v1`); nonce = `index u64 BE || last u8 ||
  0u24`; AAD = header || logical path. `ChunkedWriter` (`finish()`
  required), `ChunkedReader` (`read_chunk`, `read_range`, `read_all`),
  `seal_file` (0600 temp + fsync + rename + dir fsync), `open_file`.
- Small blobs: `seal(&[u8; 32], aad, pt)` (`nonce24 || ct || tag16`,
  XChaCha20-Poly1305), `open(..) -> Zeroizing<Vec<u8>>`.
- `Error { Auth, BadHeader, InvalidLength, ChunkSize, OutOfRange, Truncated, Io }`.

### spool-keys

- `keyslots.json`: `{"version":1,"data_key_id":"<uuid>","slots":[{"id",
  "provider","params","wrapped"}]}`, 0600, <= 64 KiB, <= 32 slots, written
  atomically. `wrapped` = base64(`nonce24 || ct32 || tag16`),
  XChaCha20-Poly1305 under the provider's KEK, AAD = `"spool-keyslot-v1" ||
  slot_id || data_key_id`.
- `KeySlots`: `load`, `create_new(path, provider) -> (KeySlots, DataKey)`,
  `add_slot`, `remove_slot`, `unlock_slot(id, provider)`, `unlock_any(&[providers])`
  (non-interactive providers first; `NoSlotUnlocked { attempts }` otherwise),
  `rotate`, `wipe`, `slots()`, `data_key_id()`. `FILE_NAME = "keyslots.json"`.
- `#[async_trait] KeyProvider { kind, interactive, enroll, unlock, destroy }`.
  `ProviderKind { SecretService, Session, Passphrase, Fido2Hmac, Tpm2 }`
  (`tpm2` parsed, not implemented). `SlotParams { SecretService{attributes},
  Session, Passphrase(PassphraseParams), Fido2Hmac(Fido2Params), Opaque }`.
- Providers: `SecretServiceProvider::new(allow_prompt)` (oo7; item
  attributes exactly `{application: "spool", slot: "<uuid>"}`, label "Spool
  clipboard history key", default collection; `allow_prompt = false` ->
  `Locked` instead of a dialog, `wait_for_unlock()`), `SessionProvider`
  (memory only), `PassphraseProvider::new(passphrase)` (Argon2id, 256 MiB,
  t = 3, p = 1, per-slot salt), `Fido2Provider::new(pin)` (`hmac-secret`,
  non-resident credential, rp `spool:clipboard`, optional `uv`,
  `devices()`, `with_device_path`, `require_uv`).
- `Error`: `NeedsSecret(kind)` (retryable; which prompt to show),
  `WrongKey`, `PinInvalid`, `PinBlocked`, `Dismissed`, `Locked`,
  `Unavailable`, `NoProvider`, `SecretMissing`, `ProviderCorrupt`,
  `NoSlotUnlocked`, `NotFound`, ...; `is_retryable()`.

### spool-search

Tantivy 0.26, encrypted at rest, never logs indexed text or queries; the
picker channel is its only client.

- `IndexDoc { id, change_seq, text, mime, app, tags, created_at, pinned }`
  (text beyond `MAX_INDEXED_TEXT` = 1 MiB not indexed).
- `SearchIndex::open(dir, &DataKey, &IndexIdentity) -> OpenOutcome { Ready(Opened
  { search, indexer, last_change_seq }), NeedsRebuild(RebuildReason {
  Missing, DecryptFailure, SchemaMismatch, DbMismatch, Corrupt }) }`; `Err`
  only for `Error::Locked` (`<dir>.lock`) and I/O. `rebuild(..)` builds
  `<dir>.rebuild` and swaps with `RENAME_EXCHANGE`. `in_memory()` for the
  pre-unlock session.
- `SearchIndex` (`Clone + Send + Sync`): `search(q, &Filters, limit, offset)`,
  `search_at(.., now_ms)` -> `Vec<Hit { id, score }>`, rank params.
  `Indexer` (single writer): `upsert`, `delete`, `note_change_seq`,
  `commit_if_due` (`COMMIT_BATCH` = 300 ms), `commit`.
- Queries: typing mode (every completed word must match exactly or within
  edit distance 1 for 4+ chars; the last word is a prefix; words of 3+
  chars also match mid-word via the trigram field `text_tri`, never across
  word boundaries; ranking exact > prefix > substring > fuzzy). Explicit
  mode when the query starts with `-`, contains `"`, or uses
  `text:|app:|mime:|tag:|pinned:` (AND, phrases, `-`, `OR`, parentheses).
  Rejected: ranges, sets, regexes, `field:*`, glued `*`, > 256 chars, > 16
  terms. Empty query = everything by recency/pin.
- `RankParams { half_life_secs: 86400, recency_weight: 2.0, pin_weight: 1.0 }`:
  `bm25 * (1 + w_r * 0.5^(age / half_life)) + w_p * pinned`.
- On disk: segment files in the chunked AEAD format (AAD = file name),
  `meta.json` / `.managed.json` sealed blobs, lock files plaintext and empty.
  `SCHEMA_VERSION = 2` (v1 opens as `SchemaMismatch` -> rebuild).

### spool-wayland

- `spawn(WaylandConfig { display, watch_primary }) -> (WaylandHandle,
  Receiver<WaylandEvent>)`; connect / missing-global errors are synchronous;
  first event `Ready(CompositorInfo { display, data_control_version,
  primary_supported })`.
- `WaylandEvent { Ready, NewSelection{selection, mimes, offer, ours},
  SelectionCleared{selection}, Fetched{offer, result}, Fatal(String) }`;
  `FetchResult = Result<Vec<(mime, bytes)>, FetchError { Timeout, TooLarge,
  OfferGone, Io }>`.
- `WaylandHandle` (Clone): `fetch(offer, mimes, per_rep_cap, total_cap,
  timeout)`, `set_selection(Selection, reps)`, `shutdown()`.
- Our own offers carry a rotating marker mime (`marker_mime(nonce)`), never
  reported in `mimes`; `ours` tells them apart.

### spool-kwin

- `KwinService::start(conn, ServiceConfig { require_kwin_sender = true })`
  owns `dev.bcnelson.spool` and serves `dev.bcnelson.spool.Kwin` at
  `/dev/bcnelson/spool`: `ActiveWindow(s app_id, s internal_id)` and
  `Show(i x, i y, s app_id, s internal_id)` -> `KwinEvent`s
  (`EVENT_QUEUE = 64`). Inputs validated (`validate`: `MAX_ID_LEN = 256`,
  no control chars, window ids look like QUuids, coordinates within
  `MAX_COORD`). `tracker()`, `stop()`.
- `KwinScript::load(conn, dir)` / `unload()` (plugin `spool`, via
  `org.kde.kwin.Scripting`), `kwin_available(conn)`,
  `keyboard_layout_index(conn)`.
- `ActiveWindowTracker`: `update`, `current`, `current_app_id`,
  `is_active`, `wait_for(window_id, timeout)`.
- Trust: same-user processes can call the service; `Show` is equivalent to
  `spoolctl show` (rate-limited), `ActiveWindow` only affects attribution
  and the paste-focus check.

### spool-paste

- `Paster::connect(&PasteConfig)` (blocking, own connection) picks
  `org_kde_kwin_fake_input` (v4+) when advertised, else
  `zwp_virtual_keyboard_manager_v1` (seat keymap uploaded, re-read before
  every paste), else `PasteError::Unsupported`. `backend() ->
  PasteBackend { FakeInput, VirtualKeyboard }`, `paste(chord, layout)`.
  `PasteHandle::spawn` runs it on its own thread.
- `KeyCodes::resolve(keymap, layout)` finds Ctrl / Shift / V / Insert in the
  current xkb keymap; `PasteChord { CtrlV, CtrlShiftV, ShiftInsert }`
  (`sequence`); `TerminalList` (`DEFAULT_TERMINALS`, `chord_for(app_id)`:
  terminals get Ctrl+Shift+V).
- `remote`: `RemotePaster::{locate, spawn, backend, paste}`,
  `serve_helper` (the `spool-paster` main loop), `PASTER_BIN`.
- `ToplevelTracker::spawn(display, on_change)` follows the activated
  toplevel through `zwlr_foreign_toplevel_manager_v1` (ids `wlr-<n>`).
- `PasteError { Unsupported, Connect, Wayland, ThreadGone }`.

### spool-compositor, spool-portal, spool-hypr

- `spool-compositor`: `type CompositorEvent = spool_kwin::KwinEvent`
  (`ActiveWindow { app_id, window_id, at }`, `Show { cursor, app_id,
  window_id }`); `EventSink::new() -> (EventSink, Receiver)` (`active_window`
  sanitises ids and feeds the tracker; `show(cursor)` targets the tracker's
  window); `CompositorKind::{Kwin, Hyprland, Sway, Other}::detect()` (env
  hint only); `#[async_trait] CursorProvider`; `Capabilities { kind,
  hotkey: HotkeySource { KwinScript, Portal, External }, focus: FocusSource
  { KwinScript, HyprlandIpc, WlrForeignToplevel, None }, cursor, paste }`.
- `spool-portal`: `PortalShortcuts::start(&dedicated_conn, PortalConfig {
  app_id: Some("dev.bcnelson.spool.daemon"), preferred_trigger: "LOGO+v" },
  EventSink, Option<Arc<dyn CursorProvider>>)` registers the app id via
  `org.freedesktop.host.portal.Registry` first (needs
  `dev.bcnelson.spool.daemon.desktop` on `$XDG_DATA_DIRS`), binds
  `spool-show` and turns `Activated` into `EventSink::show`.
  `PortalError { Unsupported, Cancelled, Portal }`; `portal_present`,
  `register_app_id`.
- `spool-hypr`: `HyprIpc::from_env()` / `from_dir`, `cursor_pos()`,
  `active_window() -> HyprWindow { address, class }`, `request(cmd)` (1 s,
  1 MiB cap), implements `CursorProvider`; `HyprFocus::spawn(ipc, sink)`
  follows `activewindow` / `activewindowv2` (app id = class, window id =
  `0x<address>`, titles never kept). `HyprError { Unsupported, Io, Timeout,
  Protocol }`.

## 9. spoold internals

Seams between the binary's modules (not a public API, but tests rely on
them).

- `keyflow`: `run_gated(Arc<dyn KeySource>, state_dir, Sender<KeyEvent>,
  OpenGate)`. `KeySource { provider(), wait_for_unlock() }`
  (`SecretServiceSource` = non-prompting provider). Per attempt: an
  unfinished rotation (`interrupted_rotation`: `keyslots.json.next` or
  `.old` present) -> fatal `INTERRUPTED_ROTATION`, nothing created or
  unlocked; `keyslots.json` missing -> `KeySlots::create_new` (fatal if a
  `history.db` exists without slots); present -> each slot of the
  provider's kind, validated by opening the store (`WrongKey` -> next).
  Retryable -> `KeyEvent::Waiting`, `wait_for_unlock`, backoff 1-60 s.
  Fatal -> `KeyEvent::Failed` (session store kept, nothing wiped). Success
  -> `KeyEvent::Opened(OpenedStore { store, hash_key, data_key, dir })`.
  `OpenGate::open` opens the store at most once per run (`GateError {
  AlreadyOpen, Store }`); the loser of a race returns quietly.
- `unlock`: `UnlockProviders { passphrase(secret), fido2(pin),
  secret_service() }` (`SystemProviders`: Argon2id, libfido2, prompting
  oo7), `attempt(dir, provider, gate) -> Attempt { Opened, AlreadyOpen,
  NeedsPin, Failed(reason) }`, `map_error`, `prompts(dir, need_pin)`.
- `orchestrator`: `KeyState { Locked, WaitingForWallet, Unlocking, Ready,
  SessionOnly, DevPlaintext, Failed(String) }` (`describe()` =
  `key_state`). `Orchestrator::new(Config, Store, W: WaylandSide)` +
  `with_key_flow`, `with_key_task`, `with_desktop`, `with_show_limiter`,
  `with_picker`; `run(wl_events, requests, shutdown)` (Wayland events polled
  before requests). On `Opened`: new `Policy`, one store job
  `merge_from_session` + swap, `remap_ids`, retention sweep, `Ready`.
  `Request { Public, Search, Thumb, Edit, Select, PickerHidden }`; picker
  requests come only from the picker channel (and test hooks).
  `REQUEST_QUEUE = 64`, `RETENTION_INTERVAL = 1 h`.
- Capture: `NewSelection{ours: false}` -> `plan` -> optional hint fetch ->
  `fetch` -> `evaluate` -> insert / drop / purge; only the newest pending
  offer per selection is tracked; stale `Fetched` results are discarded.
  The tracker's app id is the offer's `source_app` (makes `excluded_apps`
  work). `SelectionCleared` -> keep-alive re-publishes the candidate unless
  `SENSITIVE`.
- Search (`index`): in-memory index from the start (session / locked),
  back-filled; every store change is pumped to the single-writer
  `IndexActor`. On unlock the RAM index is dropped and `$STATE/index` opened
  with the data key on a blocking thread (`Ready` -> catch-up;
  `NeedsRebuild` or an index ahead of the DB -> paged rebuild, then
  catch-up). `IndexStatus { Pending, Rebuilding(progress), Memory, Disk,
  Unavailable }`. `SearchError { BadQuery, Internal }` (messages never
  contain the query). `spool-keyctl rotate` deletes the index; the next
  unlock rebuilds it.
- `Select { id, mode }`: load, reps (`PastePlain`: text only), `touch`,
  publish (counted), `record_dropped` (no keep-alive for re-published
  history). `Copy` -> `Copied`. Paste modes consume the last `Show`'s
  target; without target / backend / focus source -> `NotPasted(..)`. Else
  wait for the `NewSelection{ours}` confirming this publish (foreign offer
  first -> `ForeignOffer`; none within `ACK_TIMEOUT = 1 s` ->
  `NotConfirmed`; a newer Select -> `Superseded`), `picker.hide()`, then
  `tracker.wait_for(window, FOCUS_WAIT = 500 ms)`, chord from
  `TerminalList`, KWin layout index, paste via the helper -> `Pasted{chord}`.
- `desktop::start(&Config) -> (Desktop, DesktopLink)` (never fails): session
  bus (5 s) -> `org.kde.KWin` present -> `KwinService` + `KwinScript`
  (`$SPOOL_KWIN_SCRIPT_DIR` or `<exe>/../../share/spool/kwin-script`); else
  the generic path: `EventSink`, focus via `HyprFocus` | `ToplevelTracker` |
  none, hotkey via the portal (bound in the background) else `external`.
  `autopaste`: `PickerLauncher` (`ResidentPicker`, or `UnwiredPicker` when
  no picker is installed), `PasteSink` (`HelperSink`), `AutoPaste`.
- `ipc`: `bind(&Path) -> BoundSocket { listener, guard }`, `serve`,
  `RateLimiter`, `show_limiter()`.
- `security`: `set_private_umask`, `disable_dumpable`, PATH audit
  (`check_path_dirs`, `enforce_path_policy`), `check_peer` /
  `probe_peer_root`, `find_on_audited_path`, `pass_fd_as_3`.
- `paths`: `state_dir`, `ensure_private_dir` (fixes mode),
  `ensure_strict_private_dir` (refuses), `lock_state_dir -> StateLock`,
  `is_plaintext_sqlite`, `quarantine_plaintext_db`.
- `shutdown`: SIGTERM -> watchdog (`HARD_DEADLINE = 8 s`), key flow aborted,
  orchestrator stop (store close <= 4 s, index commit <= 3 s, background
  index work <= 2 s), desktop shutdown, Wayland stop, socket removed,
  `Runtime::shutdown_timeout(1 s)`; abandoned blocking work is crash-safe.
- Feature `test-hooks`: `SPOOL_TEST_HOOK_SOCKET` (`(q, limit)` ->
  `Result<Vec<ItemPreview>, String>`), `SPOOL_TEST_HOOK_CTL_SOCKET` (`mode:
  u8` -> `Select` of the newest item), `spoold --probe-fake-input`.
- Environment: `SPOOL_STATE_DIR`, `SPOOL_CONFIG`, `SPOOL_SOCKET`,
  `SPOOL_KWIN_SCRIPT_DIR`, `SPOOL_INSECURE_PATH_OK` (dev),
  `SPOOL_DEV_PLAINTEXT` (dev), `RUST_LOG`; flags `--log-stderr`,
  `--state-dir`, `--config`.

## 10. spoolctl and spool-keyctl

- `spoolctl`: `copy [--mime M] [FILE|-]`, `current [--raw]`, `show`,
  `pick [--raw]` (no reply timeout), `status [--json]`, `pause [--for
  SECS]`, `resume`. Exit codes: 0 ok, 1 error (daemon error / unreachable /
  no picker), 2 usage, 3 nothing (`current`: empty; `pick`: cancelled).
  Raw bytes unless stdout is a TTY (then `escape::escape_for_tty`).
  `client::Client` is also compiled into spoold's tests via `#[path]`.
- `spool-keyctl` (`status`/`list`, `add passphrase|fido2|secret-service`,
  `remove`, `rotate`, `recover`, `wipe`; `--state-dir`, `--unlock-slot`):
  takes the locks in section 3; mutating commands first unlock an existing
  slot and prove it by opening `history.db`; prompts on `/dev/tty` with echo
  off; exit codes 0 ok, 1 error, 2 usage, 3 spoold running, 4 no slot
  unlocked, 5 incomplete (secrets left, listed), 6 recovery needed, 7
  aborted. Rotation is crash-safe via `keyslots.json.next` / `.old`
  (`CrashPoint` tests per step); see README "Key management".

## 11. Security model

- **Trust boundary is the uid.** Every same-user process can reach the
  public socket and the session bus; spoold rejects only other uids,
  unknown pids and peers whose `/proc/<pid>/root` is unreadable or a
  Flatpak sandbox. Nothing on the public socket can read history except
  `Current` (newest item) and `Pick` (which needs the user to choose in the
  picker). Search, thumbnails, edits and unlock are picker-channel only.
- **Key management is not on the socket**: `spool-keyctl` runs offline with
  the daemon stopped (locks, section 3).
- **At rest**: SQLCipher DB, AEAD blob files and index, all derived from one
  data key held only by spoold (and spool-keyctl during an operation); the
  key is wrapped per slot in `keyslots.json`. Before unlock, history lives
  in an in-memory session store keyed by a random session key; nothing is
  ever wiped automatically on errors.
- **Process hardening**: umask 077, non-dumpable (no ptrace / core / proc
  reads by other same-user processes), PATH audit before anything is
  executed, helpers found only on the audited PATH, cleared environments
  for children, the systemd unit's seccomp set (`MemoryDenyWriteExecute`,
  `RestrictAddressFamilies=AF_UNIX`, `NoNewPrivileges`, ...; namespace
  directives deliberately left out, see `systemd-service.nix`).
- **Ingest policy**: password-manager hint, secret patterns, size caps, mime
  allowlist, excluded apps, pause; secrets copied with `spoolctl copy` are
  published but never stored.
- **Auto-paste** runs in the secret-free `spool-paster` (KWin cannot match a
  non-dumpable spoold to a desktop file); only paste chords cross that
  channel. Paste waits for the confirmed publish and for focus to return to
  the window that was active at Show time.
- **Logging** follows rule 3; the picker never receives an item's full text
  (previews and thumbnails only).
- **Known limits**: Slint keeps internal copies of the typed passphrase that
  cannot be wiped; the FIDO2 / passphrase slots have no external secret, so
  an old `keyslots.json` copy plus the passphrase / key still opens an old
  DB copy (snapshots); `LimitMEMLOCK` is capped by the user manager.

## 12. Compositor matrix

| Need | KWin | Hyprland | sway / other wlroots |
| --- | --- | --- | --- |
| clipboard | `ext-data-control-v1` | same | same |
| hotkey -> `Show` | KWin script (Meta+V) | GlobalShortcuts portal (xdg-desktop-portal-hyprland), `LOGO+v` | user binds `spoolctl show` (xdg-desktop-portal-wlr has no GlobalShortcuts) |
| active window | KWin script | `spool-hypr` IPC (`0x<address>`, class) | `ToplevelTracker` (`wlr-<n>`) |
| cursor | in `Show` | `j/cursorpos` | none: picker centred |
| paste | `org_kde_kwin_fake_input` via `spool-paster` + desktop file | `zwp_virtual_keyboard_v1` | `zwp_virtual_keyboard_v1` |
| picker | layer shell | layer shell | layer shell |

Without a focus source auto-paste is off (the item stays on the
clipboard). `spoolctl status` prints the detected capabilities.

## 13. Testing

README.md "Testing" has the commands, the environment gates and the manual
checks. In short: unit tests everywhere (fakes for Wayland, providers,
picker, paste sink); gated suites against headless sway
(`SPOOL_WAYLAND_TESTS=1`), nested virtual KWin (`SPOOL_KWIN_TESTS=1`), a
throwaway gnome-keyring on a private bus (`SPOOL_SECRET_SERVICE_TESTS`
harness scripts, and spoold's `*_secret_service` E2E tests), a mock or real
portal stack (`SPOOL_PORTAL_TESTS=1`, `SPOOL_PORTAL_KDE_TESTS=1`) and real
security keys (`SPOOL_FIDO2_HW_TESTS=1`); the NixOS VM test
(`nix build .#spool-test`) runs the packaged unit under systemd.
