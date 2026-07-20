# CLAUDE.md — working rules for this repo

This repo (Cyb3rD3sk) is the **AGPL-3.0 open-core** component of the CARVILON
platform (Copyright (c) 2026 Sascha Daemgen IT and More Systems; see `LICENSE`,
plus a separate commercial Professional edition — D-0056). It is developed
jointly by Sascha and Claude Code. The following rules are binding.

## Language

* **English everywhere in the repo** — docs, README, this file, code,
  comments, and commit messages. (Permanent rule from CD-02 onward.)
* German is only for the chat with Sascha, never in the repo.

## Branch & commit strategy

* **Work directly on `main`.** No feature branches, no PRs in this phase.
* **Conventional Commits.** Prefixes include:
  * `feat(shell): …`, `feat(cef): …` — functionality
  * `build: …` — build system / dependencies
  * `docs: …` — documentation
  * `fix: …`, `refactor: …`, `chore: …`
* Each ticket stage ends with **one** meaningful commit, followed by a push.
* **Push per stage, autonomously, never ask** (permanent rule from CD-06 /
  D-0013). The pre-push secret/IP grep below stays mandatory before every push;
  only the asking stops. If a push is denied by the tool permission system, note
  it in the final report and continue — do not stall a stage on it.
* `Cargo.lock` is **committed** (this is an application, not a library →
  reproducible builds, exact version pins).

## Pre-push secret/IP grep

Run this grep before every `git push`; it must come back clean (version numbers
that happen to match the IP pattern — e.g. Chromium versions — are known false
positives, so eyeball every hit):

```sh
git grep -nE "192\.168\.|10\.0\.|secret|token|passwor" -- . ':!Cargo.lock'
```

Principle: **no real IPs, hostnames, or secrets in the repo** — placeholders
only (documentation IPs such as `203.0.113.x`). CARVILON-internal addresses
never enter the history.

## Product copy (D-0044)

Product UI states what CyberDesk **does**, confidently and accurately. It
**never** names or points to a competitor (no "use Tor Browser" or equivalent,
ever), and it **never** self-deprecates. Honesty means not lying about
capabilities — not advertising alternatives. IP-anonymity copy points to
CyberDesk's own per-window Tor (and the VPN route once shipped). Bounded
technical limits, where they genuinely exist, live in internal docs
(`docs/cyberdesk-security.md`), never in UI, marketing, or demos. The README
counts as a product surface under this rule.

## No desktop scraping

Never screen-capture the user's desktop or any foreign window for verification.
Allowed for visual checks:

* the off-screen `--capture <png>` path, and
* captures of **CyberDesk running fullscreen** (it covers the whole monitor).

This rule exists because ad-hoc grabs during CD-01 accidentally caught private
windows.

## CEF binaries

* CEF binaries (`libcef.dll`, resources, locales, symbols) are **never**
  committed. They are fetched via `scripts/fetch-cef.ps1` into `vendor/cef/`
  (listed in `.gitignore`).
* The CEF version is **pinned exactly** (crate version in `Cargo.toml`, CEF
  distribution in `scripts/fetch-cef.ps1`, recorded in
  `docs/cyberdesk-decisions.md`, D-0002).

## NetGuard principle (D-0004)

No module opens its own network connections outside the future central NetGuard
layer. CEF-internal traffic is exempt until the request-filter work (Season 5).
CD-02 writes no network clients of its own.

## Method: read the crate, don't guess

Before coding against a CEF API, read the exact pinned crate source
(`cef 149.3.0`) for the real signatures. The API concepts named in a briefing
are a map; the crate is the truth.

## Briefing fidelity & decisions

* The current ticket briefing is the reference. Its requirements are followed.
* **Reasoned deviations are welcome** when they serve the goal better — but they
  are recorded in `docs/cyberdesk-decisions.md` as a numbered decision
  (`D-XXXX`, newest on top).
* On a real blocker, don't guess for hours: commit the clean intermediate state,
  document the problem and options in docs or `BLOCKER.md`, and stop.

## Documentation governance (CD-36, D-0053)

* **Claude Code (CC) owns and maintains the living technical docs** — it has the
  implementation ground truth: `cyberdesk-architecture.md`,
  `cyberdesk-security.md`, `cyberdesk-wire-format.md`,
  `cyberdesk-feature-backlog.md`, `cyberdesk-decisions.md`,
  `cyberdesk-degoogle-audit.md`.
* **Per-ticket doc Definition of Done:** every code change updates its affected
  living docs **in the same commit-set** as the change — never deferred. Each CC
  report lists which docs it touched. Briefings additionally name the docs to
  update, but CC updates any doc the change actually touches, not just the
  named ones.
* **Season protocol:** CC authors the **factual** season protocol in `seasons/`
  (what shipped per ticket, D-numbers, findings, dead ends) from its own
  reports; the Master chat + Sascha review it and add the strategic/narrative
  framing. The factual spine comes from CC so the record matches what was built.

## Platform

The current target is **Windows 11 (x64, MSVC)** only. No Linux/macOS support in
this phase; platform-specific code is `#[cfg(...)]`-gated where it will matter
later. (CD-02's OSR removes the last Windows-specific embed path — the child
HWND — after which compositing is platform-neutral.)
