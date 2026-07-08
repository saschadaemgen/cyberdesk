# CLAUDE.md — working rules for this repo

This repo is part of the proprietary CARVILON platform (Copyright (c) 2026
Sascha Daemgen IT and More Systems). It is developed jointly by Sascha and
Claude Code. The following rules are binding.

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

## Platform

The current target is **Windows 11 (x64, MSVC)** only. No Linux/macOS support in
this phase; platform-specific code is `#[cfg(...)]`-gated where it will matter
later. (CD-02's OSR removes the last Windows-specific embed path — the child
HWND — after which compositing is platform-neutral.)
