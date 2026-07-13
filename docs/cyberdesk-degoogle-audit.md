# CyberDesk - De-Google net-log audit (CD-17, D-0041; CD-26, D-0042)

Project CARVILON CyberDesk - living document - Status: 2026-07-13

This is the **measurement half** of CD-17. The enforcement (switches + prefs) is
in `src/degoogle.rs` and is applied automatically. This recipe proves it: capture
every outbound connection with a Chromium net-log and confirm **zero** unsolicited
Google/telemetry connections remain. Runs on the maintainer's machine (the sandbox
cannot do a live capture).

The claim being proven is **bounded**: the engine makes no *unsolicited* Google or
telemetry connection. Your own navigation still goes where you browse, and
necessary TLS infrastructure (OCSP/CRL to a visited site's own CA) is allowed -
see the three buckets under "Verdict".

---

## 1. Enable capture (opt-in, off by default)

Net-logging is OFF unless you name a path - nothing lands on disk in a normal run
(anti-forensic tenet). Set the env var, launch CyberDesk, exercise it, quit (ESC).
The network service writes the net-log on the **browser process** only.

PowerShell:

```powershell
# choose a scratch path OUTSIDE the repo
$env:CYBERDESK_AUDIT_NETLOG = "$env:TEMP\cyberdesk-netlog.json"

# launch (windowed is fine for the audit)
.\target\release\cyberdesk.exe --windowed

# ... run a scenario (below), then press ESC to quit cleanly so the log flushes
```

The rolling log records a line confirming capture is on
(`net-log capture ENABLED (audit mode) ...`) and, at startup, a `de-Google:
process-global kill switches` manifest line listing exactly what was enforced.

To capture more detail (still no cookies unless you ask for `IncludeSensitive`),
the standard Chromium companion switch is `--net-log-capture-mode=IncludeSensitive`;
the default mode already records every request URL and socket endpoint, which is
all the host-level audit needs.

Unset when done: `Remove-Item Env:\CYBERDESK_AUDIT_NETLOG`.

---

## 2. Scenarios

1. **Idle.** Launch, let the default windows open, navigate **nowhere**, leave it
   a few minutes, quit. This is the acceptance scenario - it must produce **zero**
   Google/telemetry connections.
2. **Representative browsing.** Fresh capture (new path). Visit a few ordinary
   sites in a **clearnet** slot and a **Tor** slot (toggle Tor per window). Quit.
   Expect only the visited sites' own traffic (+ necessary TLS infra).
3. **(Optional) baseline delta.** A capture of stock Chromium (or an earlier
   CyberDesk build without CD-17) to show what the enforcement removed. Nice
   confidence, not required for acceptance.

Each scenario = its own net-log file (don't overwrite between runs).

---

## 3. Inspect the net-log

The net-log is JSON. **Do not upload it to an online viewer** for a privacy audit -
inspect it locally.

Quick host grep (PowerShell) - list every host the capture touched:

```powershell
$log = Get-Content "$env:TEMP\cyberdesk-netlog.json" -Raw
# pull host/url-ish strings; eyeball the unique set
[regex]::Matches($log, '"(?:host|url|origin)":"[^"]+"') |
  ForEach-Object { $_.Value } | Sort-Object -Unique
```

Grep specifically for Google/telemetry endpoints - each of these must have **zero**
hits in the **idle** capture and appear in a browsing capture only if you actually
navigated there:

```
google.com            gstatic.com           googleapis.com
clients1.google.com   clients2.google.com   clients3.google.com   clients4.google.com
update.googleapis.com content-autofill.googleapis.com
safebrowsing.googleapis.com   safebrowsing.google.com
accounts.google.com   doubleclick.net       google-analytics.com
optimizationguide-pa.googleapis.com          gvt1.com   gvt2.com
```

For a structured view, the offline **Catapult netlog_viewer** (run locally from a
checkout, not the appspot upload) renders the sockets/DNS/URL-request timelines.

---

## 4. Verdict - three buckets

Classify every connection in the capture into exactly one bucket:

| Bucket | Rule | Bar |
| --- | --- | --- |
| **Unsolicited phone-home** | Google/telemetry host you did **not** navigate to | must be **zero** |
| **User navigation** | the sites you visited + their sub-resources | expected |
| **Necessary TLS infra** | OCSP/CRL to a **visited site's own CA** | allowed |

**Necessary-TLS note (so it is not mistaken for phone-home):** while browsing,
Chromium may contact a visited site's Certificate Authority for revocation
checking (OCSP responder / CRL distribution point named in that site's
certificate). This is TLS security for a site **you chose to visit**, not Google
phone-home, and CD-17 deliberately does **not** disable it (certificate
verification stays on). It appears only in the browsing capture, never idle, and
its host is the CA's (e.g. an `ocsp.*` / `crl.*` of the site's issuer), not
Google's.

---

## 5. Acceptance

1. **Idle net-log:** zero connections to any Google/telemetry host.
2. **Browsing net-log:** only visited-site traffic + necessary TLS infra; no
   Google phone-home.
3. Any necessary-TLS traffic is attributable to a visited site's CA (bucket 3),
   not to Google.

If idle shows a Google host, capture which one and cross-reference the startup
manifest + the `src/degoogle.rs` table - the vector's switch/pref either failed to
apply (look for an error line in the rolling log) or a new vector needs adding.

---

## 6. Reading the net-log correctly (CD-26 lessons)

The CD-26 investigation (D-0042) established two structural facts about
Chromium net-logs that matter when grading a capture:

1. **`events` vs `polledData`.** Only the `events` array records live traffic
   (a real request = a `URL_REQUEST` source with `REQUEST_ALIVE` /
   `URL_REQUEST_START_JOB` events, plus DNS/SSL/HTTP2 sources). The
   `polledData` section at the end is a **state dump** taken when capture
   stops - host-resolver cache, open HTTP2 sessions, and the Reporting API
   endpoint cache (`reportingInfo.clients`). A URL that appears ONLY in
   `polledData` was **not contacted** during the capture; it is persisted
   profile state (the `csp.withgoogle.com/csp/report-to/*` set was exactly
   this). Grade buckets on `events`; use `polledData` to judge what the
   profile still carries.
2. **The traffic annotation names the code path.** Each `REQUEST_ALIVE` event
   carries a `traffic_annotation` hash; `tools/traffic_annotation/summary/`
   `annotations.xml` in the Chromium tree maps it to the owning component
   (CD-26: `35565745` = `gaia_auth_list_accounts`, `109231476` =
   `aim_eligibility_fetch`). Resolve the hash BEFORE hypothesizing about
   sources - it turns "some google.com request" into a file and a service.

**Expected idle result after D-0042 (fresh or old profile):** zero
Google/telemetry hosts in `events`. `polledData[].reportingInfo` shows the
Reporting API disabled (no `clients` loaded - the persisted "Reporting and
NEL" store is no longer read). What MAY legitimately appear instead: a few
**refused connection attempts to `127.0.0.1:9`** - that is the GAIA stack
(`AccountInvestigator`'s ListAccounts) hitting the dead loopback origin the
`--gaia-url` switch pins (D-0042). Connection refused locally, zero bytes
leave the machine; it is the proof the redirect works, not a finding.
