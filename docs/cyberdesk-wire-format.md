# CyberDesk - Wire Format

Project CARVILON CyberDesk - living document - Status: 2026-07-08

Deliberately thin for now - the formats emerge from CD-02 on. Rule: every interface change is documented here before it lands on main.

## Host<->CEF IPC (planned)

- Explicit allowlist of named commands. Documented per command: name, direction, fields, error cases.
- No generic eval or passthrough channels.
- First commands arrive with CD-02: frame handover (OSR texture), input forwarding (mouse/keyboard), navigation (load URL, back/forward).

## NetGuard policy (sketch)

- Per zone: allowed destinations (host, port, protocol), optional pinning fingerprint, limits (rate, volume).
- Default: deny. Policy changes are versioned and logged.
- Format decision (file vs. SQLite) falls with the NetGuard base build.

## CARVILON protocols

- Edge and VPS integration follows in Season 7. The server documents remain authoritative (carvilon-server-wire-format.md); this document then holds only the CyberDesk view (which endpoints, which direction, which auth).
