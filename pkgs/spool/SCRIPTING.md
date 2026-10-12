# Spool scripting (design)

Status: **S1 (filters) built** 2026-10-11; S2 / S3 are still design.
INTERFACES.md ("Script socket", section 9 `scripts`, section 11) is the
reference for what is built; README "Scripts" is the user guide. Where S1
differs from the design below it is noted in "S1 as built".

## Decisions

| Question | Decision | Why |
| --- | --- | --- |
| Language | **Lua 5.4** via `mlua` (vendored, `async`, `serialize`) | familiar, small, good Rust bindings; LuaJIT is a feature swap later |
| Sandbox | **none beyond the process boundary** | scripts are the user's own; they need processes and network |
| Where scripts run | a separate host, **`spool-scripts`**, own systemd user unit | spoold's unit forbids TCP, children, W+X; the data key must never share a process with them |
| Host down / slow / script error | **keep the copy** (store it unfiltered) | losing history is worse than an unfiltered item; built-in policy still applies |

Rejected along the way: Piccolo (pure Rust, but the in-process advantage is
moot once scripts need the network), Rhai / QuickJS / Luau (their value is a
sandbox we do not need), V8 / Deno (size, Nix build, TypeScript not wanted),
plain socket clients in any language (synchronous filters across arbitrary
clients are hard to make reliable).

## Goals and non-goals

Goals:

1. **Filters**: on every copy that the built-in policy would store, scripts
   can keep it (optionally rewritten / tagged) or drop it.
2. **Reactions**: after an item is stored, scripts are told and may do
   anything (run programs, call HTTP APIs) without holding up capture.
3. **Actions**: named, user-triggered transforms on a chosen item from the
   picker, whose result becomes a derived item (like an editor save).

Non-goals: protecting the user from their own scripts; scripts reading
arbitrary history (they see items as they are stored and the item an action
was run on, nothing else); key management of any kind.

## 1. Processes

| Process | Started by | Talks to | Holds secrets |
| --- | --- | --- | --- |
| `spool-scripts` | its own systemd user unit (home-manager), `PartOf=` / `After=spoold.service` | spoold over `<runtime>/spool/scripts.sock`; whatever the scripts reach | the items it is handed, while a hook runs |

- **Not spoold's child.** A child would inherit seccomp,
  `RestrictAddressFamilies=AF_UNIX`, `MemoryDenyWriteExecute` and
  `NoNewPrivileges`; the scripts need the network and to start programs. The
  host connects to spoold as a client and reconnects (backoff 1-30 s) when
  spoold restarts.
- **Unit**: `Restart=on-failure`, `UMask=0077`, `LimitCORE=0`; deliberately
  no seccomp / address-family restrictions. The environment is the user
  manager's (programs started by `spool.exec` find the user's `PATH`).
- **Stays dumpable.** spoold authenticates the host by reading
  `/proc/<pid>/exe` (below), which needs ptrace-read access to the peer and
  fails for a non-dumpable process (the same reason `spool-paster` is the
  dumpable half of auto-paste). The host holds no key, only items in flight.

## 2. Script channel (`spool_proto::script`)

- Socket `<runtime>/spool/scripts.sock` (0600, in the existing 0700 dir),
  bound by spoold only when `[scripts] enabled = true`.
- **Peer check**: `security::check_peer` (same uid, non-Flatpak) **and**
  `readlink /proc/<pid>/exe` equals the `spool-scripts` that
  `security::find_on_audited_path` finds (`/nix/store` or root-owned, not
  writable). Anything else is rejected and logged by pid. This is what keeps
  "can read every copy" from becoming "any same-uid process can read every
  copy" (the public socket's rule in INTERFACES.md section 11). One host at a
  time; a second connection is refused while one is live.
- Framing as the public socket (`u32` BE length + postcard, `MAX_FRAME`),
  `Hello { proto: SCRIPT_PROTO_VERSION = 1 }`, host first. Enums and structs
  append-only.
- Host -> spoold:
  - `Register { filters: bool, on_stored: bool, actions: Vec<ActionInfo> }`
    (once after `Hello`; again after a reload replaces it).
    `ActionInfo { key: String, label: String, mimes: Vec<String> }`
    (`mimes` are patterns as in `[editor]`: `text/*`, `*/*`, ...).
  - `FilterDone { seq, verdict: Verdict }`;
    `Verdict { Keep { tags: Vec<String>, reps: Option<Vec<WireRep>> }, Drop }`
    (`reps = Some` = rewrite).
  - `ActionDone { seq, result: ActionResult }`;
    `ActionResult { Derived(Vec<WireRep>), Nothing, Failed }`.
  - `Copy { reps: Vec<WireRep>, tags: Vec<String> }` (`spool.copy`: publish +
    store, exactly as public `Copy`, `source_app = "spool.scripts"`).
- spoold -> host:
  - `Filter { seq, item: ScriptItem }`, `Stored { item: ScriptItem, outcome:
    Inserted|Bumped }`, `Action { seq, key, item: ScriptItem }`.
  - `ScriptItem { id: Option<i64>, selection, source_app, created_at, flags,
    tags, reps: Vec<WireRep> }` (`id = None` for a filter: not stored yet).
    Aliases are resolved: the host sees every mime the item answers to.
- No reply ever carries a reason string from the script to spoold's log:
  `Drop` and `Failed` are bare (rule 3: a script's message may quote content).

## 3. spoold side

### Filters (capture path)

- Hook point: `Orchestrator::after_fetch`, on `Decision::Store(item)`, before
  `store_item`. Scripts therefore **never see what the built-in policy
  dropped** (secrets, password-manager hints, excluded apps, paused,
  too large, not allowlisted).
- The orchestrator does **not await** the host inline (it is one task that
  also serves the picker and Wayland events). Like a fetch, the item is
  parked in `pending` with a new `Stage::Filter { seq, deadline }`; the reply
  or the deadline arrives as an event. A newer offer for the same selection
  supersedes it exactly as it supersedes a pending fetch (the parked item is
  then stored unfiltered: it was a real copy).
- **Fail open** (decision above): no host connected, host did not register
  filters, `FilterDone` later than `[scripts] filter_budget_ms` (default
  250), channel error -> `store_item(item)` unchanged. A late reply is
  discarded by `seq`. Logged: `hash_prefix`, "filter timeout" / "no host".
- `Drop` -> `record_dropped`, logged with reason `ScriptFilter`.
- `Keep { reps: Some(..) }` -> the new reps go through
  `Policy::manual_reps` (caps, secret patterns, text aliases, new dedupe
  hash), keeping `selection`, `source_app`, `created_at`, flags. A rewrite
  that now looks like a secret is dropped (`Secret(kind)`), not stored, and
  the original is not stored either (it was rewritten for a reason).
- `Keep { tags }` -> `store.set_tag` per tag in the same store job as the
  insert (tags validated as picker tags: no control characters, no `/`).
- Works the same before unlock (session store; ids remapped on merge).

### Reactions

- After `store_item` succeeds, `Stored` is queued to the host if it
  registered `on_stored`. Fire-and-forget; bounded queue (`64`), overflow
  dropped and counted in the log. Never sent for `SENSITIVE` items.

### Actions

- Picker channel v4 appends `PickerReq::Action { seq, id, key }` and
  `PickerEvt::Actions(Vec<ActionInfo>)` (pushed on connect and whenever the
  host registers); the picker shows the actions that match the selected
  item's mimes.
- spoold loads the item (as `Select` does), sends `Action`, waits up to
  `[scripts] action_timeout_ms` (default 30 000; actions may call the
  network). `Derived(reps)` -> `Policy::manual_reps` ->
  `store.insert_derived(item, Some(id))` (tags carried over, `derived_from`
  set) -> published like a picker `Copy`, exactly the editor-save path in
  INTERFACES.md section 9. `Nothing` -> no-op; `Failed` / timeout ->
  content-free notification ("Spool: action failed").
- `spoolctl action <key>` runs an action on the **current** item only (the
  public socket can already read `Current`; it still cannot name history).

### Config (`[scripts]`, `deny_unknown_fields`)

```toml
[scripts]
enabled = false
filter_budget_ms = 250     # 10..=2000
action_timeout_ms = 30000
```

## 4. The host (`crates/spool-scripts`, bin)

- tokio + `mlua` (Lua 5.4, vendored, `async`, `serialize`); one Lua state,
  hooks run as coroutines so a slow reaction does not block a filter.
- **Script set**: `spool-scripts --scripts <dir>` loads `<dir>/*.lua` in name
  order. The directory must be under `/nix/store` (home-manager builds it)
  unless `SPOOL_INSECURE_SCRIPTS_OK=1` (development, like
  `SPOOL_INSECURE_PATH_OK`): a writable script dir would let any same-uid
  process install a clipboard logger. Reload = unit restart (home-manager
  `X-Restart-Triggers` on the dir).
- A script that fails to load is skipped and logged (file name and line);
  the others still run.
- **Filter chain**: every registered filter in load order; the first `drop`
  wins; rewrites and tags compose (each filter sees the previous one's
  output). A Lua error in a filter counts as `keep` unchanged for that
  filter. The host also cancels a filter coroutine at the budget so a hung
  one does not pile up.
- **Logging**: script errors are logged as `file:line` and the error class
  only; the full message (which may contain content) only with
  `SPOOL_SCRIPTS_DEBUG=1`. `spool.log` writes to the journal as given: what a
  script logs is its author's choice.

### Lua API (`spool` global)

```lua
-- hooks
spool.filter(function(item) ... end)            -- return spool.keep{...} | spool.drop() | nil (= keep)
spool.on_stored(function(item) ... end)
spool.action{ key = "json-pretty", label = "Pretty-print JSON",
              mimes = { "text/*" }, run = function(item) ... end }
                                                 -- return { text = "..." } | { reps = {...} } | nil

spool.keep{ tags = { "work" }, text = "..." }    -- or reps = { ["text/html"] = "...", ... }
spool.drop()

-- item
item.id, item.selection, item.source_app, item.created_at, item.tags, item.flags
item.mimes                                       -- list
item.text                                        -- preferred text rep as a string, or nil
item:get("image/png")                            -- bytes (Lua string) or nil

-- effects (async: yield the coroutine, never block the host)
spool.exec{ "jq", ".", stdin = s, timeout = 5 } -- { code, stdout, stderr }
spool.http{ method = "POST", url = u, headers = {...}, body = b, timeout = 10 }
                                                 -- { status, headers, body }
spool.copy("text")  /  spool.copy{ reps = {...}, tags = {...} }
spool.notify("summary", "body")
spool.log("message")

-- helpers (Lua patterns are too weak for clipboard text)
spool.regex(pat):match(s) / :find_all(s) / :replace(s, rep)
spool.json.decode(s) / spool.json.encode(v)
spool.url.parse(s) / spool.url.build(t)
```

The Lua standard library is fully loaded (`io`, `os` included): there is no
sandbox by decision. A `spool.d.lua` definitions file ships with the package
for lua-language-server completion.

Example: strip tracking parameters and tag work copies.

```lua
local tracking = spool.regex("^(utm_.*|fbclid|gclid|mc_eid)$")

spool.filter(function(item)
  local tags = {}
  if item.source_app == "org.kde.slack" then tags[#tags + 1] = "work" end

  local text = item.text
  if text and text:match("^https?://%S+$") then
    local u = spool.url.parse(text)
    local kept = {}
    for _, kv in ipairs(u.query) do
      if not tracking:is_match(kv[1]) then kept[#kept + 1] = kv end
    end
    u.query = kept
    return spool.keep{ tags = tags, text = spool.url.build(u) }
  end
  return spool.keep{ tags = tags }
end)
```

## 5. home-manager

```nix
services.spool.scripts = {
  enable = true;
  filterBudgetMs = 250;
  files = {
    "10-urls.lua" = ./spool/urls.lua;    # path or { text = "..."; }
  };
};
```

Builds the script dir into the store, writes `[scripts]` into `config.toml`,
adds the `spool-scripts` unit (restart-triggered by the dir), and puts the
binary on spoold's audited `PATH` so the peer check can find it.

## 6. Security model delta

- New reader of content: the host sees every item the policy stores, and the
  item an action runs on. It never sees the data key, history it was not
  handed, or anything the policy dropped.
- Trust anchor: the host binary path (audited, store) and the script dir
  (store). Same-uid processes still cannot read history through spoold; they
  could already do anything the scripts do.
- Script-written content re-enters only through `Policy::manual_reps`
  (filters, actions) or the public-`Copy` path (`spool.copy`), so caps and
  secret patterns always apply.
- Known limits: the host is dumpable (required for the exe check), so a
  same-uid process can read items in flight from its memory; scripts can
  exfiltrate anything they are handed, by design.

## 7. Testing

- `spool-proto`: round-trip / append-only tests for the new enums.
- `spoold`: a fake host on the script socket (no Lua) covers fail-open on
  no host / timeout / disconnect mid-filter, drop, rewrite through
  `manual_reps` (rewrite into a secret is dropped), tags, supersession by a
  newer offer, peer rejection of a non-audited exe, action -> derived item.
- `spool-scripts`: Lua unit tests against an in-process fake spoold;
  `spool.exec` against `/bin/sh`; `spool.http` against a local hyper server
  (sandbox: loopback only, no network).
- VM test: extend `nixos-test.nix` with the home-manager option, a filter
  that tags and a filter that drops, asserted via `spoolctl`.

## S1 as built

- `Register { filters }` only; `on_stored` / `actions` come with S2 / S3
  (the script protocol version is bumped then; host and daemon ship in one
  package).
- `ScriptEvt::Filter` carries `budget_ms`; the host spends at most 80 % of
  it (instruction-count hook every 1000 instructions) and answers "keep
  unchanged" when it runs out, the same thing spoold does without an
  answer. A Lua error in one filter skips that filter only. A filter
  blocked inside a C function (`io.read`, `os.execute`) is not
  interrupted; spoold's deadline covers it, later copies queue behind it.
- `ScriptItem` has canonical `reps` plus `aliases` (names only) instead of
  every mime with data; `item:get` resolves aliases. After a rewrite the
  next filter sees only the new reps (spoold adds the text aliases back).
- `spool.keep{ text = .. }` replaces the whole item with one
  `text/plain;charset=utf-8` rep; `reps = { {mime, data} | {mime = ..,
  data = ..}, .. }` replaces it with those (first is canonical).
- `spool.url.parse` returns `query` as a list of `{key, value}` pairs (URL
  order matters) plus the raw `query_string`; `build` re-encodes the query
  only if one of them changed.
- `print` is routed to `spool.log`. `spool.json.null` is the JSON null.
- The host does blocking I/O on one thread (no tokio yet; S2's async
  `exec` / `http` will need it). Memory cap 256 MiB; a script's top level
  gets 5 s to load. `spool-scripts --check` loads the scripts and exits;
  the home-manager module runs it at build time (`scripts.check`).
- Copies are finished strictly in copy order (FIFO); a copy that no host
  took while others are parked queues behind them.
- `spoolctl copy` and editor saves are not filtered.

## Versions (built 2026-10-11)

A rewrite keeps the copy as it came in as a hidden earlier version of the
rewritten item (`spool.keep{ .., original = false }` opts out, for
redaction; one such filter in a chain is enough), noted `script:<files>`.
Editor saves joined the same model: the edit becomes the current item and
the item it came from a version of it. Versions are not listed, searched,
deduped against or counted by retention, and go when their item goes.
Browse: the picker's versions view, `spoolctl versions`, `spoolctl current
--original | --version N`. Store side: INTERFACES.md `spool-core` "Versions".

## 8. Milestones

1. **S1** (built): proto + script socket + peer check, host with `spool.filter`,
   `keep` / `drop` / rewrite / tags, fail-open, regex / json / url helpers.
2. **S2**: `spool.on_stored`, `exec`, `http`, `notify`, `copy`, `log`.
3. **S3**: actions (picker v4, `spoolctl action`), `spool.d.lua`, VM test.

## Open questions

1. Fail open on **script errors and timeouts** too, not only when the host is
   down? (Built that way, for consistency.)
2. Should the dropped original be stored when a rewrite is refused as a
   secret? (Built: no.)
3. Picker key for the action menu.
4. Should filters see the primary selection (when `primary_selection =
   true`), or the clipboard only? (Built: both; `item.selection` tells.)
