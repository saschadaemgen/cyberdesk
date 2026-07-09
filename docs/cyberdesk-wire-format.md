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
- Success: `{"feather_edges":<bool>,"animated_background":<bool>,"stay_foreground":<bool>,"glow_intensity":<int>,"search_engine":<str>}`
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

### `get_nav_state` (view -> host)

- Request: `{"cmd":"get_nav_state"}`
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

- Request: `{"cmd":"navigate","input":"<str>"}`
- `input` is the raw command-bar text; the host classifies it (URL vs. search):
  - contains `://` -> used verbatim (an explicit `http://` stays http)
  - `localhost` (optionally `:port`/`/path`), or a dot with no whitespace ->
    `https://<input>`
  - empty -> `about:blank`
  - otherwise -> `https://www.google.com/search?q=<urlencoded>` (or the selected
    search engine, CD-07)
- Effect: loads the resolved URL in the active slot (spawning its browser if the
  slot is still lazy, CD-09) and slides the top bar away (CD-08; a committed
  navigation is one of the bar's hide triggers).
- Success: `{"ok":true,"url":"<resolved-url>"}`
- Failure: code 1 (malformed request), 2 (missing `input`).

### `go_back` / `go_forward` / `reload` (view -> host)

- Request: `{"cmd":"go_back"}` | `{"cmd":"go_forward"}` | `{"cmd":"reload"}`
- Effect: the active slot steps back / forward in session history, or reloads.
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

## NetGuard policy (sketch)

- Per zone: allowed destinations (host, port, protocol), optional pinning fingerprint, limits (rate, volume).
- Default: deny. Policy changes are versioned and logged.
- Format decision (file vs. SQLite) falls with the NetGuard base build.

## CARVILON protocols

- Edge and VPS integration follows in Season 7. The server documents remain authoritative (carvilon-server-wire-format.md); this document then holds only the CyberDesk view (which endpoints, which direction, which auth).
