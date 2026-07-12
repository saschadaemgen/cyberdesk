# CyberDesk - Wire Format

Project CARVILON CyberDesk - living document - Status: 2026-07-09

Deliberately thin for now - the formats emerge from CD-02 on. Rule: every interface change is documented here before it lands on main.

## Host<->CEF IPC (planned)

- Explicit allowlist of named commands. Documented per command: name, direction, fields, error cases.
- No generic eval or passthrough channels.
- First commands arrive with CD-02: frame handover (OSR texture), input forwarding (mouse/keyboard), navigation (load URL, back/forward).

## Settings IPC (CD-03, live)

The internal settings view (`cyberdesk://settings/`) talks to the Rust host over
the CEF message router (`window.cefQuery`) — process messages, never network
requests. The bridge is registered ONLY on `cyberdesk://` contexts, so it exists
only on the internal view; the surf zone has no access to it.

Transport: `window.cefQuery({ request, persistent: false, onSuccess, onFailure })`.
`request` is a JSON string; the success response is a JSON string; failures carry
`(error_code, message)`. Commands are an explicit allowlist — no generic eval.

### `get_settings` (view -> host)

- Request: `{"cmd":"get_settings"}`
- Success: `{"feather_edges":<bool>,"animated_background":<bool>,"stay_foreground":<bool>,"glow_intensity":<int>,"search_engine":<str>,"tor_enabled":<bool>,"tor_default":<bool>}`
  (`tor_enabled` / `tor_default` added in CD-15.)
  - `glow_intensity` is a whole percent (50..=220).
  - `search_engine` ∈ { `google`, `duckduckgo`, `bing`, `startpage` } (CD-07).
- Failure: code 1 (malformed request JSON).

### `set_setting` (view -> host)

- Request: `{"cmd":"set_setting","key":"<key>","value":<bool|int|str>}`
- Writable keys and their value types:
  - `feather_edges`, `animated_background`, `stay_foreground` — boolean.
  - `glow_intensity` — number (whole percent; accepts a JSON number or a numeric
    string, clamped host-side to 50..=220).
  - `search_engine` — string (CD-07); one of `google`, `duckduckgo`, `bing`,
    `startpage`. Any other value is rejected with code 3.
- Effect: updates the in-memory setting (applied by the next rendered frame /
  next navigation) and the SQLite `settings` row (survives restart).
- Success: `{"ok":true,"key":"<key>","value":<bool|int|str>}`
- Failure: code 1 (malformed request), 2 (missing `key`/`value` or wrong type),
  3 (unknown key or unknown `search_engine` value), 4 (unknown `cmd`).

CD-05 (D-0012) renamed the background toggle `deep_field` -> `animated_background`
(it now governs whichever background the template selects) and added the numeric
`glow_intensity`; the store migrates the old key. Unknown commands are rejected
with code 4. There is no passthrough channel.

## Command bar / navigation IPC (CD-04, live)

The command bar view (`cyberdesk://command/`) drives navigation over the same
message-router bridge as the settings view (`window.cefQuery`, process messages
only, registered on `cyberdesk://` contexts only). Since CD-09 (D-0017) every
command here targets the **active slot** — the host reads/drives
`browser::active_slot()` internally — rather than a single fixed surf view; the
top bar always shows and drives the active column. This needed **no new commands
and no field changes**: the wire format below is unchanged from CD-08, only the
host-side target moved from `Role::Surf` to the active slot. The internal views
are never navigated through this channel. Error codes share the single space
defined above (1 = malformed request JSON, 2 = missing/wrong-typed field, 4 =
unknown `cmd`).

CD-10 (sessions, width units, rearrange, open-in-new-slot; D-0018/D-0019) added
**no IPC** either: session persistence is store-side, the width/rearrange
shortcuts are host-side key handling, and open-in-new-slot is a CEF
`on_before_popup` decision (not a page command). The wire format is unchanged.

CD-11 (the main frame — side zones, reflow-to-rails; D-0020) is likewise **no
IPC**: the frame is pure host-side layout math (`slots::frame_layout`) and shell
rendering. The wire format is unchanged. The **revised frame law (D-0022** — three
slots, a permanent right Multifunctional zone, a flexible left Spine zone, gutter
56) is also **no IPC**: it only changes `frame_layout`'s pure math and the shell
glyphs. The CD-12 `cdFrame` push below carries the resulting slot rects verbatim,
so the floating layer adapts with no wire change.

CD-14 (own start page, no saved websites; D-0025) adds **no new commands**. The
own start page (`cyberdesk://start/`, the default content of every empty slot) is
served from the binary and reuses the existing `navigate` (search / address box)
and `query_suggestions ""` (favorite tiles) commands below — both act on the
active slot (interacting with a slot activates it). The one wiring change: the
browser-side message router now forwards `on_process_message_received` for **every**
view, not just the internal one, so a slot's start page can use `window.cefQuery`.
This is safe — `cefQuery` is exposed only on `cyberdesk://` frames (the render-side
`on_context_created` gate), and the start page is the sole `cyberdesk://` content a
slot ever shows (a web page in a slot has no query bridge). Session-URL persistence
is removed (store-side, no IPC).

CD-12 (floating command sets; D-0021) **retires the single top bar**. The command
view becomes N floating **ensembles** (one per column) plus a shared favorites
launcher, so every navigation command below now accepts an **optional `slot`
field** — the id of the ensemble's column. The host's `target_slot` reads it
(clamped to a live slot), else falls back to the keyboard-active slot (so an
omitted `slot` preserves the CD-09 behaviour). No commands were removed and no
fields renamed; `slot` is purely additive. Two new pieces — the host→page frame
push (`window.cdFrame`) and `drag_start` — are documented under "Floating command
sets IPC" below.

### `get_nav_state` (view -> host)

- Request: `{"cmd":"get_nav_state"[,"slot":<int>]}` — `slot` (CD-12) selects the
  ensemble's column; omitted → the active slot. Each ensemble's capsule shows its
  own column's url/title/scheme/star.
- Success: `{"url":<str>,"title":<str>,"can_back":<bool>,"can_forward":<bool>,"loading":<bool>,"scheme":<str>,"favorite":<bool>,"autofocus":<bool>}`
  - `scheme` ∈ { `https`, `http`, `other` }, derived host-side from `url`. The
    command bar paints the amber "insecure" hint when `scheme == "http"`.
  - `favorite` (CD-07) is whether `url` is currently a favorite; it drives the
    star glyph in the command bar.
  - `autofocus` (CD-08) tells the bar page whether to focus + select its input on
    this open: `true` for a Ctrl+L reveal, `false` for a hover-to-top reveal
    (which shows the favorites chips without stealing the caret). Set host-side
    before each reveal.
- Failure: code 1 (malformed request JSON).

### `navigate` (view -> host)

- Request: `{"cmd":"navigate","input":"<str>"[,"slot":<int>]}` — `slot` (CD-12)
  targets the ensemble's column; omitted → the active slot.
- `input` is the raw command-bar text; the host classifies it (URL vs. search):
  - contains `://` -> used verbatim (an explicit `http://` stays http)
  - `localhost` (optionally `:port`/`/path`), or a dot with no whitespace ->
    `https://<input>`
  - empty -> `about:blank`
  - otherwise -> `https://www.google.com/search?q=<urlencoded>` (or the selected
    search engine, CD-07)
- Effect: loads the resolved URL in the target slot (spawning its browser if the
  slot is still lazy, CD-09) and disengages the command band (CD-12; a committed
  navigation is one of the band's hide triggers).
- Success: `{"ok":true,"url":"<resolved-url>"}`
- Failure: code 1 (malformed request), 2 (missing `input`).

### `go_back` / `go_forward` / `reload` (view -> host)

- Request: `{"cmd":"go_back"[,"slot":<int>]}` | `{"cmd":"go_forward"[,"slot":<int>]}`
  | `{"cmd":"reload"[,"slot":<int>]}` — `slot` (CD-12) targets the ensemble's
  column; omitted → the active slot.
- Effect: the target slot steps back / forward in session history, or reloads.
  Back/forward are no-ops when `can_back` / `can_forward` (from `get_nav_state`)
  is false.
- Success: `{"ok":true}`
- Failure: code 1 (malformed request JSON).

The F5 / Ctrl+R reload and Ctrl+Shift+R hard reload accelerators are handled
host-side from the shell key map, not over this channel.

## Command palette IPC (CD-07, live)

The command bar is a command palette: it shows live suggestions from the local
favorites + history store (D-0014) and toggles favorites. Same transport and
error-code space as above (internal `cyberdesk://command/` view only). All local
— no network.

### `query_suggestions` (view -> host)

- Request: `{"cmd":"query_suggestions","input":"<str>"}`
  - `input` is the current command-bar text; a missing `input` is treated as empty.
- Success: a JSON array (0..=`command.max_results` items), best first:
  `[{"url":<str>,"title":<str>,"favorite":<bool>}, …]`
  - Ranking (host-side): matching favorites first (in their saved order), then
    matching history by frecency (see D-0014). Empty `input` returns the top
    favorites. Matching is a case-insensitive substring on url + title.
  - CD-08: the bar renders an empty-`input` reply as the favorites **chip** row
    (up to `command.max_results` chips) and a non-empty reply as the suggestion
    **list**. Host-side, the reply length plus whether `input` was empty size the
    bar (`input row + chip row`, or `input row + N suggestion rows`). Only one of
    the two bodies is ever populated, so the height stays exact.
- Failure: code 1 (malformed request JSON).

### `toggle_favorite` (view -> host)

- Request: `{"cmd":"toggle_favorite","url":"<str>","title":"<str>"}`
  - A missing `title` defaults to empty. Internal `cyberdesk://` / blank URLs are
    ignored (they are never favorited).
- Effect: adds the URL to favorites (appended at the end) or removes it.
- Success: `{"favorite":<bool>}` — the new state (true = now a favorite).
- Failure: code 1 (malformed request), 2 (missing `url`).

The surf-view **Ctrl+D** toggles the current page's favorite host-side (from the
shell key map) rather than over this channel; the palette's star and its Ctrl+D
use `toggle_favorite`.

## Top bar IPC (CD-08, live)

The command surface is now a hover-reveal **top bar** (D-0016). Its reveal/hide
animation is entirely host-side; the page adds one signal so a typing user is not
interrupted by a mouse-out. Chips reuse the empty-`input` `query_suggestions`
above — there is no separate favorites command.

### `bar_typing` (view -> host)

- Request: `{"cmd":"bar_typing","active":<bool>}`
  - `active` = the bar's input is focused **and** holds text (the prefilled URL
    counts). The page reports it on the input's focus / blur / input events, only
    when the value changes.
- Effect: while `active` is true the host's mouse-out hysteresis will not hide the
  bar. It is reset to `false` host-side on every reveal (a fresh page reports its
  own state), so a stale value cannot wedge the bar open.
- Success: `{"ok":true}`
- Failure: code 1 (malformed request JSON). A missing `active` defaults to false.

The reveal (hover into the top gap, or Ctrl+L which sets `autofocus`) and the hide
(mouse-out + ~250 ms hysteresis, a committed `navigate`, or ESC — with the typing
exception above) are decided host-side; see docs/cyberdesk-decisions.md D-0016.

## Floating command sets IPC (CD-12, live)

The bar retires into N transparent floating **ensembles** on one internal band view
plus a shared favorites **launcher** (D-0021). The band still uses the message-router
bridge (`cyberdesk://command/`, process messages, allowlisted) for the nav/palette
commands above (now with the `slot` field). CD-12 adds one host→page push, one pull,
and one page→host command. Same error-code space (1 malformed, 2 missing/typed, 4
unknown `cmd`).

### `cdFrame(json)` (host -> view, push)

Not a `cefQuery` — the host calls `window.cdFrame(<json-string>)` on the band view
via `Frame::execute_java_script` whenever the frame state **changes** (engaged slot
or any column's target x/width), never per frame (the CD-11 on-change cadence). The
page positions/reveals its ensembles from it and glides via CSS (~220 ms).

- Payload: `{"slots":[{"id":<int>,"x":<num>,"w":<num>}, …],"engaged":<int|null>,"autofocus":<bool>}`
  - `slots` — one entry per live column in display order; `x`/`w` are **band-DIP**
    (the band's origin = the window origin), so the page places each ensemble above
    its column. `id` is the stable slot id (the same id the `slot` field carries back).
  - `engaged` — the id of the column whose ensemble is revealed, or `null` (all hidden).
  - `autofocus` — focus + select the engaged capsule's input on this reveal (Ctrl+L).
    A **transient**: it is excluded from the host's change-signature, so a routine
    position-only push cannot clear a pending focus.

### `get_frame` (view -> host, pull)

- Request: `{"cmd":"get_frame"}`
- Effect: the page pulls the last-pushed frame state once on load (the band view can
  reload independently of the host's push), then relies on `cdFrame` pushes.
- Success: the current frame-state JSON string (the same payload as `cdFrame`), or an
  empty string if none has been pushed yet.
- Failure: code 1 (malformed request JSON).

### `drag_start` (view -> host)

- Request: `{"cmd":"drag_start","url":"<str>","title":"<str>"}`
  - Fired by a favorites launcher **tile** once the pointer leaves a 6 px threshold
    (a plain click still navigates the engaged column). A missing `title` defaults
    to empty.
- Effect: the host **takes over the drag** — it draws a ghost circle on the cursor
  and the control gutters as drop zones, captures the mouse (slot views receive no
  events), and on release inserts + spawns the favorite as a new column at the
  nearest gutter, or (at full capacity) navigates the slot under the ghost. ESC
  cancels. All of this is host-side; the page's only role is this one signal.
- Success: `{"ok":true}`
- Failure: code 1 (malformed request), 2 (missing `url`).

The per-slot **close orb** (a shell-drawn ring + cross revealed on top-outer-corner
hover, a click closes that column) is **no IPC** — it is drawn by the renderer and
hit-tested host-side, like the gear button.

### `toggle_tor` (view -> host, CD-15)

- Request: `{"cmd":"toggle_tor"[,"slot":<int>]}` — the ensemble's Tor shield glyph.
  `slot` is the column id; omitted → the active slot.
- Effect: queues a per-window Tor flip for the main thread, which tears the slot's
  browser down and respawns it under the other request context (Tor: a proxied
  per-slot context; clearnet: the direct global context) at the start page. No-op if
  the engine master switch (`tor_enabled`) is off and the target is turning Tor ON.
- Success: `{"ok":true}`. Failure: code 1 (malformed request JSON).

### `close_slot` (view -> host, CD-18)

- Request: `{"cmd":"close_slot"[,"slot":<int>]}` — the ensemble's close icon. `slot`
  is the column id; omitted → the active slot.
- Effect: queues a per-window close for the main thread, which enforces
  last-slot-refuses + neighbor promotion (`close_slot_at`, the single choke point
  shared with `Ctrl+W`). Replaces the retired CD-12 shell-drawn corner close orb.
- Success: `{"ok":true}`. Failure: code 1 (malformed request JSON).

### `quit` / `quit_save` (view -> host, CD-21)

- Request: `{"cmd":"quit"}` or `{"cmd":"quit_save"}` — the two floating quit buttons
  in the MF-zone view (`cyberdesk://mfzone/`). **Application-level** quit (they end
  the whole shell), deliberately distinct from `close_slot` (one window).
- Effect: sets a `pending_quit` flag `Some(false)` (`quit`) / `Some(true)`
  (`quit_save`) for the main thread; the drain in `about_to_wait` calls
  `event_loop.exit()` (so `shutdown_cef` runs after `run_app` returns). `quit_save`
  first persists the session (schema v6 `session_slots` + the `session_savequit`
  restore flag): per slot the mode (Tor/clearnet), width, active, order, and — for
  clearnet slots only — the URL (Tor-slot and internal/blank URLs are stored empty
  for privacy, D-0035). Plain `quit` writes nothing → the next launch is the default
  two-slot layout. The IPC handler runs on the CEF UI thread and never touches the
  event loop directly.
- Success: `{"ok":true}`. Failure: code 1 (malformed request JSON).

The **frame-state push** (`cdFrame`, CD-12) gained a per-slot `"tor":<bool>` field
and a top-level `"tor_status":<int>` (0 off / 1 connecting / 2 ready / 3 failed), so
the per-window Tor icon lights when the column is on Tor, pulses while the engine
bootstraps (1), reads a distinct **ready** state (2), and warns on **failed** (3).

## Per-window Tor + settings IPC (CD-15, live)

### `tor_status` (view -> host)

- Request: `{"cmd":"tor_status"}` (the settings page + the MF-zone Tor tab poll it).
- Success: `{"status":<int>,"reason":<str>,"version":<str>}` — `status` 0 off (not
  started) / 1 bootstrapping / 2 ready / 3 failed; `reason` the failure reason
  (empty unless failed, CD-15 HOTFIX); `version` the embedded arti-client version
  (CD-18). Failure: code 1 (malformed request JSON).

### `tor_new_circuit` (view -> host, CD-18)

- Request: `{"cmd":"tor_new_circuit"}` (the settings "New circuit" button).
- Effect: bumps a "new identity" epoch so each per-slot SOCKS relay rebuilds its
  isolated Tor client on its next connection — subsequent streams ride fresh
  circuits under a fresh isolation group (reload a page to use its new circuit). A
  lock-free atomic bump; never touches the proxy or the fail-closed guarantee.
- Success: `{"ok":true}`. Failure: code 1 (malformed request JSON).

### Tor settings (via the existing `get_settings` / `set_setting`)

Two boolean settings join the `get_settings` reply and are writable via
`set_setting` (the generic boolean path, D-0014): `tor_enabled` (engine master
switch, default `true`) and `tor_default` (open new windows on Tor, default
`false`). No new command — the wire shape is the CD-03 settings channel.

## MF-zone viewer IPC (CD-18, live)

The MF-zone viewer (`cyberdesk://mfzone/`) uses the same message-router bridge as
settings/info. Its Tor tab reuses `tor_status`; its Tor + Log tabs stream the log
ring buffer via one command.

### `get_log_lines` (view -> host)

- Request: `{"cmd":"get_log_lines"[,"since_seq":<int>][,"filter":{"target_prefix":<str>,"level_min":<str>}]}`.
  `since_seq` returns only records with a **strictly higher** `seq` (incremental
  polling — the page sends back the highest seq it has seen). **Omit** `since_seq`
  for the whole buffer (sending `0` would drop the record with `seq` 0).
  `filter.target_prefix` keeps records whose `tracing` target starts with the prefix
  (e.g. `tor_`, `cyberdesk::tor`); `filter.level_min` is a min severity word
  (`trace`/`debug`/`info`/`warn`/`error`).
- Success: a JSON array `[{"seq":<int>,"ts":<int ms>,"level":<str>,"target":<str>,"msg":<str>}, ...]`
  oldest→newest, from the in-memory ring buffer (last ~2000 records; the file log is
  separate). The ring copies only the message field — never other structured fields,
  so no secrets leak into the viewer. Failure: code 1 (malformed request JSON).

## Update-awareness IPC (CD-13 → CD-22, live)

The info panel (`cyberdesk://info/`) uses the same message-router bridge
(`window.cefQuery`, process messages, internal view only) as settings / command.
**Read-only now:** every component status is derived CLIENT-SIDE (installed version
vs a build-declared latest-known version), so the panel has ONE command,
`get_info_items`, and opens no network of its own. The CD-13 `dismiss_item` /
`check_updates` commands and the live manifest fetch were **retired in CD-22**
(D-0036) — the app self-update feed returns in its own later ticket. The info glyph
itself is **no IPC** (shell-drawn, hit-tested host-side like the gear).

### `get_info_items` (view -> host)

- Request: `{"cmd":"get_info_items"}`
- Success: the info snapshot — `{"components":[…]}`.
  - `components` (CD-22) — the component list, one object per tracked component
    (`cyberdesk`, `cef`, `tor`), each
    `{"id":<str>,"name":<str>,"version":<str>,"latest":<str|null>,"status":<str>,"detail":<str|null>,"reason":<str?>,"note":<str?>}`:
    - `version` — the installed/running version, from its single existing source
      (CyberDesk: `CARGO_PKG_VERSION`; CEF: the pinned crate's compile-time constants;
      Tor: the arti-client version injected from `Cargo.lock` by `build.rs`, D-0029).
    - `latest` — the client-declared **latest-known** version for this component
      (`updates::COMPONENTS`), bumped whenever the dependency is. `null` only for an
      undeclared component.
    - `status` — the comparison of installed vs `latest`, one of **`current`**
      (installed ≥ latest-known → up to date), **`update`** (a newer, non-held-back
      version is known), **`held_back`** (a newer version is known but deliberately
      NOT installed, with `reason` + `note` — the arti 0.44 case, D-0034), or
      **`informational`** (defensive fallback for an *undeclared* component only:
      bare version, no claim). Every tracked component reports a real status — never
      a bare "INSTALLED".
    - `detail` — an optional secondary line (e.g. `"Chromium 149.0.7827.201"` for
      CEF), or `null`.
    - `reason` / `note` — present only on `held_back`. The latest-known and held-back
      values are client-side (build-time), not from a live server.
- Failure: code 1 (malformed request JSON).

## Update manifest schema — DEFERRED (CD-13/D-0023, retired in CD-22/D-0036)

The live manifest feed (`carvilon.com/updates/...` + hosting) and the app's own
self-update are **deferred to a dedicated later ticket** and are NOT fetched today
(CD-22). Until then the panel is driven entirely client-side (see `get_info_items`
above), CyberDesk shows clearly-marked demo data (`updates::COMPONENTS`), and there
is no "Last check failed" footer. The manifest JSON format below is retained as the
reference for that future ticket (sample `docs/updates/cyberdesk.sample.json`); it is
not consumed by the current build.

```json
{
  "schema": 1,
  "cyberdesk": { "latest": "0.9.0", "notes_url": "https://carvilon.com/updates/notes/0.9.0.html" },
  "components": {
    "cef": { "recommended": "150.0.1+chromium-150.0.7900.100", "reason": "security", "notes_url": "https://carvilon.com/updates/notes/cef-150.html" },
    "tor": { "recommended": "0.45.0", "reason": "security", "notes_url": "https://carvilon.com/updates/notes/tor-0.45.html" }
  }
}
```

- `schema` (int) — the manifest schema version. `cyberdesk.latest` (str) — the newest
  published CyberDesk version; `cyberdesk.notes_url` (str, optional) — release notes.
- `components` (map, optional) — keyed by component id (`cef`, `tor`), each with
  `recommended` (str), `reason` (str, optional), `notes_url` (str, optional).
- When the feed returns, the app self-update ticket will reconcile it with the
  client-side latest-known table (the client table stays the source of truth for
  held-back versions; the pin is a build-time decision, D-0034).

## NetGuard policy (sketch)

- Per zone: allowed destinations (host, port, protocol), optional pinning fingerprint, limits (rate, volume).
- Default: deny. Policy changes are versioned and logged.
- Format decision (file vs. SQLite) falls with the NetGuard base build.

## CARVILON protocols

- Edge and VPS integration follows in Season 7. The server documents remain authoritative (carvilon-server-wire-format.md); this document then holds only the CyberDesk view (which endpoints, which direction, which auth).
