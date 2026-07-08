# CyberDesk - Wire Format

Project CARVILON CyberDesk - living document - Status: 2026-07-08

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
- Success: `{"feather_edges":<bool>,"deep_field":<bool>}`
- Failure: code 1 (malformed request JSON).

### `set_setting` (view -> host)

- Request: `{"cmd":"set_setting","key":"<key>","value":<bool>}`
- `key` ∈ { `feather_edges`, `deep_field` } (the only writable keys).
- Effect: updates the in-memory toggle (applied by the next rendered frame) and
  the SQLite `settings` row (survives restart).
- Success: `{"ok":true,"key":"<key>","value":<bool>}`
- Failure: code 1 (malformed request), 2 (missing `key`/`value` or wrong type),
  3 (unknown key), 4 (unknown `cmd`).

Unknown commands are rejected with code 4. There is no passthrough channel.

## NetGuard policy (sketch)

- Per zone: allowed destinations (host, port, protocol), optional pinning fingerprint, limits (rate, volume).
- Default: deny. Policy changes are versioned and logged.
- Format decision (file vs. SQLite) falls with the NetGuard base build.

## CARVILON protocols

- Edge and VPS integration follows in Season 7. The server documents remain authoritative (carvilon-server-wire-format.md); this document then holds only the CyberDesk view (which endpoints, which direction, which auth).
