# CyberDesk update manifest - publishing

CyberDesk shows update availability in its top-right info area by fetching **one**
small static JSON - the update manifest - over HTTPS. This is the host's only
outbound connection (NetGuard exception, D-0023). Nothing is downloaded or
installed; the manifest only tells the running build what the latest versions are.

## What to publish

A single JSON file matching the schema below. A ready sample is in this folder:
[`cyberdesk.sample.json`](cyberdesk.sample.json).

```json
{
  "schema": 1,
  "cyberdesk": {
    "latest": "0.9.0",
    "notes_url": "https://carvilon.com/updates/notes/0.9.0.html"
  },
  "components": {
    "cef": {
      "recommended": "150.0.1+chromium-150.0.7900.100",
      "reason": "security",
      "notes_url": "https://carvilon.com/updates/notes/cef-150.html"
    },
    "tor": {
      "recommended": "0.45.0",
      "reason": "security",
      "notes_url": "https://carvilon.com/updates/notes/tor-0.45.html"
    }
  }
}
```

- `schema` - always `1` for now. (Adding fields later is safe; older builds ignore
  unknown fields.)
- `cyberdesk.latest` - the newest CyberDesk version you have released (semver, e.g.
  `0.9.0`). `notes_url` is optional (a page the user can open from the panel).
- `components.cef.recommended` - the CEF version you recommend (the
  `major.minor.patch+chromium-…` string). `reason` is a short word; `security`
  shows the item with the amber security accent. `notes_url` is optional.
- `components.tor.recommended` - the embedded Tor engine (arti-client) version you
  recommend (plain semver, e.g. `0.45.0` - no `+chromium` tail). `reason` and
  `notes_url` behave exactly as for `cef`. An outdated Tor client is
  security-critical, so keep this current.

An item only appears when the published version is **newer** than what the user is
running. Set the versions to what you have actually released.

## Where to publish (one upload)

1. Put the file at the URL CyberDesk expects:
   `https://carvilon.com/updates/cyberdesk.json`
   (this is the default `updates.feed_url` token; change the token if you host it
   elsewhere).
2. On the nginx webspace, that is simply the file at
   `…/updates/cyberdesk.json`. Ensure:
 - it is served over **HTTPS** with a valid certificate (CyberDesk uses rustls;
     no self-signed certs);
 - the `Content-Type` is `application/json` (nginx does this by extension);
 - it is publicly readable (no auth) - the manifest is public product info.
3. That is the whole deploy: **one file upload.** To announce a new version, edit
   the numbers and re-upload. No CyberDesk change is needed.

Any `notes_url` pages (release notes) are ordinary HTML you can host anywhere;
they open in a normal CyberDesk column when the user clicks "Release notes".

## Before it is live / offline

Until the manifest exists (404) or if the network is unreachable, CyberDesk is
silent: the glyph stays quietly idle, the panel shows the running versions as
"up to date / no feed data yet", and startup is unaffected. Nothing errors.

## Testing without publishing

Point CyberDesk at a local file with the `CYBERDESK_UPDATE_FEED` environment
variable (a documented test affordance):

```pwsh
$env:CYBERDESK_UPDATE_FEED = "C:\Projects\Carvilon\cyberdesk\docs\updates\cyberdesk.sample.json"
cargo run --release
```

The sample's versions are ahead of the current build, so the info glyph fills with
a count and the panel lists the CyberDesk, CEF and Tor items. Unset the variable to
return to the real endpoint.
