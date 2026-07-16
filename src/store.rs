//! Persistent application state (SQLite via rusqlite).
//!
//! A schema-versioned `settings` key/value table plus a `meta` table holding the
//! selected `template` (only value: "cyber"). CD-07 (D-0014) adds the `history`
//! and `favorites` tables — the local memory behind the command palette. CD-10
//! (D-0019) adds `session_slots` — the persisted slot workspace. Lives in the OS
//! app-data directory, never in the repo.

// Some store methods are consumed only by specific IPC paths (settings, memory);
// keep the surface complete even where a given build doesn't touch every method.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use rusqlite::Connection;

const SCHEMA_VERSION: i64 = 7;

/// History is capped at this many rows; the oldest are pruned on each insert
/// (D-0014). Local only — no sync, no export.
const HISTORY_CAP: i64 = 10_000;

/// One command-palette suggestion row: a favorite or a history entry.
pub struct Suggestion {
    pub url: String,
    pub title: String,
    pub favorite: bool,
}

/// One persisted session slot (CD-21, D-0035): a slot's display position, the URL
/// it showed, its width in units, whether it was the active slot, and its Tor mode.
/// Only an explicit "Quit & Save" writes these; a plain quit leaves the restore
/// flag clear so the next launch is the default layout. Internal/blank URLs and
/// Tor-slot URLs are stored empty (privacy, D-0025) — the caller filters them.
pub struct SessionSlot {
    pub position: i64,
    pub url: String,
    pub width_units: i64,
    pub active: bool,
    pub tor: bool,
}

pub struct Store {
    conn: Connection,
}

/// The process-wide store, opened on first use. The settings IPC (settings.rs)
/// and the history/favorites layer (memory.rs) share this one connection behind
/// a single Mutex — one `state.db`, one lock.
pub fn shared() -> &'static Mutex<Store> {
    static S: OnceLock<Mutex<Store>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Store::open()))
}

fn data_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("APPDATA"))
        .unwrap_or_else(|_| ".".to_string());
    let dir = PathBuf::from(base).join("CyberDesk");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

impl Store {
    pub fn open() -> Self {
        Self::from_connection(
            Connection::open(data_dir().join("state.db")).expect("failed to open state.db"),
        )
    }

    /// An isolated in-memory store for tests — a throwaway temp DB with the same
    /// schema (migrated) and defaults (seeded) as the real `state.db`, but no
    /// filesystem and no shared global. The regression harness drives the real
    /// history/favorites code against one of these.
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Self {
        Self::from_connection(
            Connection::open_in_memory().expect("failed to open in-memory state.db"),
        )
    }

    fn from_connection(conn: Connection) -> Self {
        let store = Self { conn };
        // CD-33 (D-0050): pin the temp schema to RAM before anything can use it. This
        // is what makes the session's `history` table (create_ram_history) memory-only,
        // and it also keeps SQLite from spilling sorter/index scratch for ANY query
        // into a temp FILE next to the database — an anti-forensic win beyond history.
        store
            .conn
            .pragma_update(None, "temp_store", "MEMORY")
            .expect("failed to pin sqlite temp storage to memory");
        store.migrate();
        store.create_ram_history();
        store.seed_defaults();
        store
    }

    fn migrate(&self) {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap_or(0);
        if version < 1 {
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS settings (
                         key   TEXT PRIMARY KEY,
                         value TEXT NOT NULL
                     );
                     CREATE TABLE IF NOT EXISTS meta (
                         key   TEXT PRIMARY KEY,
                         value TEXT NOT NULL
                     );",
                )
                .expect("failed to create schema");
        }
        if version < 2 {
            // CD-07 (D-0014): local history + favorites. `url` is the identity in
            // both tables (upsert on revisit / re-favorite); history is capped at
            // HISTORY_CAP rows, favorites keep an explicit order via `position`.
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS history (
                         url         TEXT PRIMARY KEY,
                         title       TEXT NOT NULL DEFAULT '',
                         last_visit  INTEGER NOT NULL,
                         visit_count INTEGER NOT NULL DEFAULT 1
                     );
                     CREATE INDEX IF NOT EXISTS idx_history_last_visit
                         ON history (last_visit);
                     CREATE TABLE IF NOT EXISTS favorites (
                         url      TEXT PRIMARY KEY,
                         title    TEXT NOT NULL DEFAULT '',
                         added_at INTEGER NOT NULL,
                         position INTEGER NOT NULL
                     );",
                )
                .expect("failed to migrate to schema v2 (history + favorites)");
        }
        if version < 3 {
            // CD-10 (D-0019): the persisted slot workspace. One implicit session;
            // `position` is the display order, rewritten wholesale on each save.
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS session_slots (
                         position    INTEGER PRIMARY KEY,
                         url         TEXT NOT NULL DEFAULT '',
                         width_units INTEGER NOT NULL DEFAULT 1,
                         active      INTEGER NOT NULL DEFAULT 0
                     );",
                )
                .expect("failed to migrate to schema v3 (session_slots)");
        }
        if version < 4 {
            // CD-13 (D-0023): update awareness. `update_meta` caches the last-known
            // manifest JSON + the last check time (so the glyph reflects last-known
            // offline); `update_dismissed` holds, per info item id, the target
            // version it was dismissed at — the item re-appears only if the manifest
            // later advances past it.
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS update_meta (
                         key   TEXT PRIMARY KEY,
                         value TEXT NOT NULL
                     );
                     CREATE TABLE IF NOT EXISTS update_dismissed (
                         id      TEXT PRIMARY KEY,
                         version TEXT NOT NULL
                     );",
                )
                .expect("failed to migrate to schema v4 (update awareness)");
        }
        if version < 5 {
            // CD-14 (D-0025): websites are not saved. Drop the session workspace
            // table — this also PURGES any open-website URLs a prior build had
            // persisted (the privacy reversal of CD-10/D-0019). Slots always start
            // fresh at the own start page.
            self.conn
                .execute_batch("DROP TABLE IF EXISTS session_slots;")
                .expect("failed to migrate to schema v5 (drop session_slots)");
        }
        if version < 6 {
            // CD-21 (D-0035): the session workspace RETURNS — now opt-in and
            // mode-aware. Re-creates session_slots with a per-slot `tor` column (the
            // mode CD-10 lacked). The v5 DROP above already removed the old
            // 4-column table, so this is a clean CREATE (never an ALTER of a shape
            // that no longer exists); a DB at v3/v4 runs both in this one pass.
            // Restore is gated by a `meta` flag set ONLY by "Quit & Save"; a plain
            // quit leaves it clear → default layout. Privacy is preserved: internal/
            // blank slots and Tor slots persist an empty URL (the caller filters),
            // so no browsed URL reaches disk unless the user opts in via Quit & Save.
            self.conn
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS session_slots (
                         position    INTEGER PRIMARY KEY,
                         url         TEXT NOT NULL DEFAULT '',
                         width_units INTEGER NOT NULL DEFAULT 1,
                         active      INTEGER NOT NULL DEFAULT 0,
                         tor         INTEGER NOT NULL DEFAULT 0
                     );",
                )
                .expect("failed to migrate to schema v6 (session_slots + per-slot mode)");
        }
        if version < 7 {
            // CD-33 (D-0050): history is BROWSING CONTENT, so it must not live on
            // disk. Drop the persisted table — which also PURGES every URL + title a
            // prior build recorded — and re-create it per session in RAM (see
            // `create_ram_history`). Same shape as the v5 drop (D-0025), for the same
            // reason: the privacy reversal has to take the existing rows with it, or
            // the residue outlives the decision.
            //
            // `favorites` deliberately stays on disk: a favorite is an explicit user
            // act (Ctrl+D), not a trace of where you have been — the bookmark/history
            // split every ephemeral browser makes.
            self.conn
                .execute_batch(
                    "DROP INDEX IF EXISTS idx_history_last_visit;
                     DROP TABLE IF EXISTS history;",
                )
                .expect("failed to migrate to schema v7 (history off disk)");
        }
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .ok();
    }

    /// Create this session's RAM-only `history` table (CD-33, D-0050).
    ///
    /// A TEMP table lives in the connection's temp schema, which `temp_store =
    /// MEMORY` (set in [`Store::from_connection`]) pins to RAM — so history is never
    /// written to disk and dies with the process, exactly like the cache and cookies
    /// now do. SQLite resolves an unqualified name against temp BEFORE main, so every
    /// existing history query keeps working untouched; there is no `main.history` left
    /// for them to hit (v7 dropped it).
    fn create_ram_history(&self) {
        self.conn
            .execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS history (
                     url         TEXT PRIMARY KEY,
                     title       TEXT NOT NULL DEFAULT '',
                     last_visit  INTEGER NOT NULL,
                     visit_count INTEGER NOT NULL DEFAULT 1
                 );
                 CREATE INDEX IF NOT EXISTS temp.idx_history_last_visit
                     ON history (last_visit);",
            )
            .expect("failed to create the in-memory history table");
    }

    fn seed_defaults(&self) {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO meta (key, value) VALUES ('template', 'cyber')",
                [],
            )
            .ok();
        self.set_if_absent("feather_edges", "true");
        // CD-05 (D-0012) renamed the background toggle `deep_field` ->
        // `animated_background`. Carry the old on/off value across if present,
        // so a user who had disabled the background keeps it disabled.
        if self.get("animated_background").is_none() {
            let prev = self.get("deep_field");
            self.set("animated_background", prev.as_deref().unwrap_or("true"));
        }
        self.set_if_absent("stay_foreground", "true");
        // CD-07: the command-bar search-engine choice. CD-27 (D-0043) flipped
        // the factory default Google -> DuckDuckGo: a de-Googled browser must
        // not ship Google as its default search.
        self.set_if_absent("search_engine", "duckduckgo");
        // CD-27 (D-0043) one-shot migration: every pre-CD-27 store carries a
        // LITERAL "google" row written by this seeder — indistinguishable from
        // a user choice, so flipping it corrects our own seed, not the user.
        // The meta marker limits the flip to exactly one run: an EXPLICIT
        // post-migration Google choice sticks across restarts.
        if self.meta_get("search_default_cd27").is_none() {
            if self.get("search_engine").as_deref() == Some("google") {
                self.set("search_engine", "duckduckgo");
            }
            self.meta_set("search_default_cd27", "done");
        }
        // CD-15: the Tor engine is available by default; new windows are clearnet.
        self.set_if_absent("tor_enabled", "true");
        self.set_if_absent("tor_default", "false");
        // CD-25: the global fingerprinting-hardening preset, default Standard (the
        // recommended coherent default). Per-window overrides are session-ephemeral
        // and are not persisted here; the custom per-vector blob (hardening_custom)
        // is written only when the user enters custom mode.
        self.set_if_absent("hardening_level", "standard");
        // glow_intensity is seeded lazily from the background.glow_default token
        // in settings::init (kept out of the store until the user changes it).
    }

    fn set_if_absent(&self, key: &str, value: &str) {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO settings (key, value) VALUES (?1, ?2)",
                (key, value),
            )
            .ok();
    }

    pub fn template(&self) -> String {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = 'template'", [], |r| {
                r.get(0)
            })
            .unwrap_or_else(|_| "cyber".to_string())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .ok()
    }

    pub fn get_bool(&self, key: &str, default: bool) -> bool {
        self.get(key).map(|v| v == "true").unwrap_or(default)
    }

    pub fn set(&self, key: &str, value: &str) {
        self.conn
            .execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
            .ok();
    }

    pub fn set_bool(&self, key: &str, value: bool) {
        self.set(key, if value { "true" } else { "false" });
    }

    /// All settings as (key, value) pairs, for the get_settings IPC command.
    pub fn all_settings(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Ok(mut stmt) = self.conn.prepare("SELECT key, value FROM settings")
            && let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            out.extend(rows.filter_map(Result::ok));
        }
        out
    }

    // --- Session workspace (CD-21, D-0035) ----------------------------------
    //
    // A single implicit session, mode-aware, opt-in. `save_session` writes it on an
    // explicit "Quit & Save"; `take_saved_session` restores it ONCE at launch and
    // consumes the flag, so a later plain quit / crash boots the default layout. The
    // restore flag lives in the `meta` table (not `settings`, so it never surfaces in
    // the settings-page all_settings IPC).

    fn meta_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .ok()
    }

    fn meta_set(&self, key: &str, value: &str) {
        self.conn
            .execute(
                "INSERT INTO meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
            .ok();
    }

    fn session_save_flag(&self) -> bool {
        self.meta_get("session_savequit").as_deref() == Some("1")
    }

    fn set_session_save_flag(&self, on: bool) {
        self.meta_set("session_savequit", if on { "1" } else { "0" });
    }

    /// Persist the current session (an explicit "Quit & Save"). The restore flag is
    /// cleared FIRST and only re-set after the rows commit, so a crash mid-write
    /// falls back to the default layout rather than restoring a partial session.
    pub fn save_session(&self, slots: &[SessionSlot]) {
        self.set_session_save_flag(false);
        if self.write_session_rows(slots) {
            self.set_session_save_flag(true);
        }
    }

    /// Replace all session rows in one transaction. Returns whether it committed.
    fn write_session_rows(&self, slots: &[SessionSlot]) -> bool {
        // Store methods hold `&self` (the connection is shared behind the store
        // Mutex), so use `unchecked_transaction` — the Mutex already serialises access.
        let Ok(tx) = self.conn.unchecked_transaction() else {
            return false;
        };
        if tx.execute("DELETE FROM session_slots", []).is_err() {
            return false;
        }
        for s in slots {
            if tx
                .execute(
                    "INSERT INTO session_slots (position, url, width_units, active, tor)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    (s.position, &s.url, s.width_units, s.active as i64, s.tor as i64),
                )
                .is_err()
            {
                return false;
            }
        }
        tx.commit().is_ok()
    }

    fn load_session_rows(&self) -> Vec<SessionSlot> {
        let mut out = Vec::new();
        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT position, url, width_units, active, tor
             FROM session_slots ORDER BY position ASC",
        ) && let Ok(rows) = stmt.query_map([], |r| {
            Ok(SessionSlot {
                position: r.get(0)?,
                url: r.get(1)?,
                width_units: r.get(2)?,
                active: r.get::<_, i64>(3)? != 0,
                tor: r.get::<_, i64>(4)? != 0,
            })
        }) {
            out.extend(rows.filter_map(Result::ok));
        }
        out
    }

    /// Take the saved session IFF the last quit was a "Quit & Save": returns the
    /// slots (ordered by position) and CONSUMES the flag — a one-shot restore, so a
    /// later plain quit or a crash boots the default layout (D-0035). `None` when
    /// there is nothing to restore (plain quit, first run, or an old/unknown schema
    /// whose migration left the table empty).
    pub fn take_saved_session(&self) -> Option<Vec<SessionSlot>> {
        if !self.session_save_flag() {
            return None;
        }
        self.set_session_save_flag(false);
        let rows = self.load_session_rows();
        // One-shot: consume the ROWS too, not just the flag — so no saved URL lingers
        // on disk past the restore. Otherwise the last Quit & Save URLs would sit in
        // state.db until the next Quit & Save overwrote them, even after a plain quit
        // (which means "don't keep this"). Matches the D-0025 purge doctrine.
        let _ = self.conn.execute("DELETE FROM session_slots", []);
        if rows.is_empty() { None } else { Some(rows) }
    }

    // --- History (D-0014) ---------------------------------------------------

    /// Record a visit to `url`: insert a new row, or bump the existing row's
    /// `visit_count` and `last_visit`. A non-empty `title` refreshes the stored
    /// one; an empty title (address change before the page's title arrives) is
    /// left untouched. Prunes the oldest rows past the cap afterwards.
    pub fn record_visit(&self, url: &str, title: &str) {
        self.conn
            .execute(
                "INSERT INTO history (url, title, last_visit, visit_count)
                 VALUES (?1, ?2, CAST(strftime('%s','now') AS INTEGER), 1)
                 ON CONFLICT(url) DO UPDATE SET
                     last_visit  = CAST(strftime('%s','now') AS INTEGER),
                     visit_count = visit_count + 1,
                     title       = CASE WHEN excluded.title <> ''
                                        THEN excluded.title ELSE history.title END",
                (url, title),
            )
            .ok();
        self.prune_history();
    }

    /// Refresh the stored title of an existing history row without bumping the
    /// visit count (the page title usually arrives after the address commit).
    pub fn update_history_title(&self, url: &str, title: &str) {
        self.conn
            .execute("UPDATE history SET title = ?2 WHERE url = ?1", (url, title))
            .ok();
    }

    /// Drop the least-recently-visited rows beyond `HISTORY_CAP`.
    fn prune_history(&self) {
        self.conn
            .execute(
                "DELETE FROM history WHERE url IN (
                     SELECT url FROM history
                     ORDER BY last_visit DESC, rowid DESC
                     LIMIT -1 OFFSET ?1
                 )",
                [HISTORY_CAP],
            )
            .ok();
    }

    // --- Favorites (D-0014) -------------------------------------------------

    /// Is `url` a favorite?
    pub fn is_favorite(&self, url: &str) -> bool {
        self.conn
            .query_row("SELECT 1 FROM favorites WHERE url = ?1", [url], |_| Ok(()))
            .is_ok()
    }

    /// Toggle `url`'s favorite state; returns the new state (true = now a
    /// favorite). New favorites append at the end of the ordered list.
    pub fn toggle_favorite(&self, url: &str, title: &str) -> bool {
        if self.is_favorite(url) {
            self.conn
                .execute("DELETE FROM favorites WHERE url = ?1", [url])
                .ok();
            false
        } else {
            let position: i64 = self
                .conn
                .query_row(
                    "SELECT COALESCE(MAX(position), -1) + 1 FROM favorites",
                    [],
                    |r| r.get(0),
                )
                .unwrap_or(0);
            self.conn
                .execute(
                    "INSERT INTO favorites (url, title, added_at, position)
                     VALUES (?1, ?2, CAST(strftime('%s','now') AS INTEGER), ?3)",
                    (url, title, position),
                )
                .ok();
            true
        }
    }

    // --- Update awareness (D-0023) ------------------------------------------

    fn update_meta_set(&self, key: &str, value: &str) {
        self.conn
            .execute(
                "INSERT INTO update_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                (key, value),
            )
            .ok();
    }

    fn update_meta_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM update_meta WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .ok()
    }

    /// The last-known manifest JSON (cached so the info glyph reflects last-known
    /// state offline / before the first successful check).
    pub fn cached_manifest(&self) -> Option<String> {
        self.update_meta_get("manifest")
    }

    pub fn set_cached_manifest(&self, json: &str) {
        self.update_meta_set("manifest", json);
    }

    /// Unix seconds of the last update check attempt (success or failure), or None.
    pub fn last_update_check(&self) -> Option<i64> {
        self.update_meta_get("last_check")
            .and_then(|s| s.trim().parse::<i64>().ok())
    }

    pub fn set_last_update_check(&self, secs: i64) {
        self.update_meta_set("last_check", &secs.to_string());
    }

    /// Record that info item `id` was dismissed at `version` (the target version at
    /// dismissal time); re-inserting updates it. The item stays hidden until the
    /// manifest advances past `version`.
    pub fn dismiss_update(&self, id: &str, version: &str) {
        self.conn
            .execute(
                "INSERT INTO update_dismissed (id, version) VALUES (?1, ?2)
                 ON CONFLICT(id) DO UPDATE SET version = excluded.version",
                (id, version),
            )
            .ok();
    }

    /// Every dismissed item id → the version it was dismissed at.
    pub fn dismissed_updates(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Ok(mut stmt) = self.conn.prepare("SELECT id, version FROM update_dismissed")
            && let Ok(rows) =
                stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        {
            out.extend(rows.filter_map(Result::ok));
        }
        out
    }

    // --- Suggestions (D-0014) -----------------------------------------------

    /// Command-palette suggestions for `input`, capped at `limit`: matching
    /// favorites first (by their order), then matching history by frecency.
    /// Empty input returns the top favorites.
    ///
    /// Frecency = `visit_count * recency_weight`, where the weight is bucketed
    /// by the age of the last visit: <1 h → 100, <1 day → 80, <1 week → 60,
    /// <30 days → 40, else → 20. Deliberately simple and honest; favorites
    /// always outrank history (D-0014). Matching is a case-insensitive substring
    /// on url + title.
    pub fn query_suggestions(&self, input: &str, limit: usize) -> Vec<Suggestion> {
        let mut out: Vec<Suggestion> = Vec::new();
        let trimmed = input.trim();

        if trimmed.is_empty() {
            // Empty input: the top favorites in their explicit order.
            if let Ok(mut stmt) = self
                .conn
                .prepare("SELECT url, title FROM favorites ORDER BY position ASC LIMIT ?1")
                && let Ok(rows) = stmt.query_map([limit as i64], |r| {
                    Ok(Suggestion { url: r.get(0)?, title: r.get(1)?, favorite: true })
                })
            {
                out.extend(rows.filter_map(Result::ok));
            }
            return out;
        }

        let pattern = format!("%{}%", like_escape(&trimmed.to_lowercase()));

        // Matching favorites first (their order).
        if let Ok(mut stmt) = self.conn.prepare(
            "SELECT url, title FROM favorites
             WHERE lower(url) LIKE ?1 ESCAPE '\\' OR lower(title) LIKE ?1 ESCAPE '\\'
             ORDER BY position ASC LIMIT ?2",
        ) && let Ok(rows) = stmt.query_map((pattern.as_str(), limit as i64), |r| {
            Ok(Suggestion { url: r.get(0)?, title: r.get(1)?, favorite: true })
        }) {
            out.extend(rows.filter_map(Result::ok));
        }

        // Then matching history by frecency, excluding anything already a favorite.
        let remaining = limit.saturating_sub(out.len());
        if remaining > 0
            && let Ok(mut stmt) = self.conn.prepare(
                "SELECT url, title FROM history
                 WHERE (lower(url) LIKE ?1 ESCAPE '\\' OR lower(title) LIKE ?1 ESCAPE '\\')
                   AND url NOT IN (SELECT url FROM favorites)
                 ORDER BY (visit_count * CASE
                       WHEN (CAST(strftime('%s','now') AS INTEGER) - last_visit) < 3600    THEN 100
                       WHEN (CAST(strftime('%s','now') AS INTEGER) - last_visit) < 86400   THEN 80
                       WHEN (CAST(strftime('%s','now') AS INTEGER) - last_visit) < 604800  THEN 60
                       WHEN (CAST(strftime('%s','now') AS INTEGER) - last_visit) < 2592000 THEN 40
                       ELSE 20 END) DESC, last_visit DESC
                 LIMIT ?2",
            )
            && let Ok(rows) = stmt.query_map((pattern.as_str(), remaining as i64), |r| {
                Ok(Suggestion { url: r.get(0)?, title: r.get(1)?, favorite: false })
            })
        {
            out.extend(rows.filter_map(Result::ok));
        }

        out
    }
}

/// Escape LIKE wildcards so user input matches literally (paired with `ESCAPE
/// '\'`), so a typed `%` or `_` doesn't act as a wildcard.
fn like_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for c in s.chars() {
        if c == '\\' || c == '%' || c == '_' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CD-33 (D-0050): history must live in the RAM-only temp schema, never on disk.
    /// Asserted through sqlite's own catalogs rather than by trusting the pragma:
    /// `temp.sqlite_master` must own `history`, and `main` must not.
    #[test]
    fn history_lives_in_ram_not_on_disk() {
        let s = Store::open_in_memory();

        let in_temp: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM temp.sqlite_master WHERE type='table' AND name='history'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(in_temp, 1, "history must be a TEMP (in-memory) table");

        let in_main: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM main.sqlite_master WHERE type='table' AND name='history'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(in_main, 0, "history must NOT exist in the on-disk schema");

        // temp_store=MEMORY (2) is what keeps the temp schema off the filesystem; a
        // file-backed temp store would put history right back on disk.
        let temp_store: i64 = s
            .conn
            .query_row("PRAGMA temp_store", [], |r| r.get(0))
            .unwrap();
        assert_eq!(temp_store, 2, "sqlite temp storage must be pinned to MEMORY");
    }

    /// A visit still records and still surfaces in the palette — history works
    /// exactly as before within the session; only its persistence is gone.
    #[test]
    fn ram_history_still_records_and_suggests() {
        let s = Store::open_in_memory();
        s.record_visit("https://example.org/page", "A Page");

        let hits = s.query_suggestions("example", 6);
        assert_eq!(hits.len(), 1, "the visit must be suggestible in-session");
        assert_eq!(hits[0].url, "https://example.org/page");
        assert!(!hits[0].favorite);
    }

    /// The v7 migration must PURGE a prior build's on-disk history, not just stop
    /// writing new rows — residue that outlives the decision defeats the point.
    /// Drives a real v6-shaped database with a row already in it.
    #[test]
    fn v7_migration_purges_previously_persisted_history() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE history (
                 url TEXT PRIMARY KEY, title TEXT NOT NULL DEFAULT '',
                 last_visit INTEGER NOT NULL, visit_count INTEGER NOT NULL DEFAULT 1
             );
             INSERT INTO history (url, title, last_visit, visit_count)
                 VALUES ('https://old.example/visited-page', 'Old', 1, 3);
             PRAGMA user_version = 6;",
        )
        .unwrap();

        // Sanity: the row is really there in the on-disk schema before migrating.
        let before: i64 = conn
            .query_row("SELECT count(*) FROM main.history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, 1);

        let s = Store::from_connection(conn);

        let leftover: i64 = s
            .conn
            .query_row(
                "SELECT count(*) FROM main.sqlite_master WHERE type='table' AND name='history'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftover, 0, "the persisted history table must be dropped");
        // And the session starts with an empty RAM history — the old URL is gone.
        assert_eq!(s.query_suggestions("old.example", 6).len(), 0);
    }

    /// Two distinct pages favorited in sequence must both persist (no collapse to
    /// one), and the empty-input palette query — the surface the reveal shows —
    /// must return both, in their saved order. This is the storage half of the
    /// CD-08 favorites repro (the display half is fixed in command.js).
    #[test]
    fn two_distinct_favorites_persist_and_list() {
        let s = Store::open_in_memory();
        assert!(s.toggle_favorite("https://a.example/", "Alpha"));
        assert!(s.toggle_favorite("https://b.example/", "Beta"));

        let all = s.query_suggestions("", 6);
        assert_eq!(all.len(), 2, "both favorites must survive");
        assert_eq!(all[0].url, "https://a.example/");
        assert_eq!(all[1].url, "https://b.example/");
        assert!(all.iter().all(|x| x.favorite));
    }

    /// The insert must not use REPLACE/UPSERT semantics that would overwrite an
    /// existing favorite — re-favoriting the same URL toggles it off, and only
    /// that one.
    #[test]
    fn toggle_off_removes_only_that_favorite() {
        let s = Store::open_in_memory();
        s.toggle_favorite("https://a.example/", "Alpha");
        s.toggle_favorite("https://b.example/", "Beta");
        assert!(!s.toggle_favorite("https://a.example/", "Alpha")); // now un-favorited

        let all = s.query_suggestions("", 6);
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].url, "https://b.example/");
    }

    /// Typing a query that matches only one favorite returns that one — this is
    /// what the old palette did on open with the current URL prefilled, and why
    /// only a single favorite ever showed. The empty-input list (above) is the
    /// contrast that proves both are stored.
    #[test]
    fn filtered_query_narrows_to_matching_favorite() {
        let s = Store::open_in_memory();
        s.toggle_favorite("https://a.example/", "Alpha");
        s.toggle_favorite("https://b.example/", "Beta");

        let only_b = s.query_suggestions("b.example", 6);
        assert_eq!(only_b.len(), 1);
        assert_eq!(only_b[0].url, "https://b.example/");
    }

    // --- Search-engine factory default (CD-27, D-0043) ----------------------

    /// A fresh store seeds DuckDuckGo as the search engine — never Google
    /// (CD-27 acceptance 2, headless half).
    #[test]
    fn fresh_store_defaults_to_duckduckgo() {
        let s = Store::open_in_memory();
        assert_eq!(s.get("search_engine").as_deref(), Some("duckduckgo"));
        assert!(s.meta_get("search_default_cd27").is_some(), "marker set on first seed");
    }

    /// A pre-CD-27 store (the seeder's literal "google" row, no migration
    /// marker) is flipped to DuckDuckGo by the next open's seed pass.
    #[test]
    fn seeded_google_row_migrates_to_duckduckgo() {
        let s = Store::open_in_memory();
        // Rewind to the pre-CD-27 state: google row, marker absent.
        s.set("search_engine", "google");
        s.conn
            .execute("DELETE FROM meta WHERE key = 'search_default_cd27'", [])
            .unwrap();
        s.seed_defaults(); // what the next open runs
        assert_eq!(s.get("search_engine").as_deref(), Some("duckduckgo"));
    }

    /// An EXPLICIT post-migration Google choice survives the next open — the
    /// marker limits the flip to one run; Google stays a working option.
    #[test]
    fn explicit_google_choice_survives_reopen() {
        let s = Store::open_in_memory();
        s.set("search_engine", "google"); // the user picked Google in settings
        s.seed_defaults(); // next open: marker already present, no flip
        assert_eq!(s.get("search_engine").as_deref(), Some("google"));
    }

    /// A non-Google pre-CD-27 choice is never touched by the migration.
    #[test]
    fn non_google_choice_is_untouched_by_migration() {
        let s = Store::open_in_memory();
        s.set("search_engine", "bing");
        s.conn
            .execute("DELETE FROM meta WHERE key = 'search_default_cd27'", [])
            .unwrap();
        s.seed_defaults();
        assert_eq!(s.get("search_engine").as_deref(), Some("bing"));
    }

    // --- Session save/restore (CD-21, D-0035) -------------------------------

    /// A "Quit & Save" session restores exactly once (mode, url, width, active
    /// preserved) and is then consumed — a second launch (or a plain quit) boots
    /// the default layout.
    #[test]
    fn save_quit_session_restores_once_then_defaults() {
        let s = Store::open_in_memory();
        assert!(s.take_saved_session().is_none(), "nothing saved yet → default boot");

        let rows = vec![
            SessionSlot { position: 0, url: "https://a.example/".into(), width_units: 1, active: true, tor: false },
            SessionSlot { position: 1, url: String::new(), width_units: 2, active: false, tor: true },
        ];
        s.save_session(&rows);

        let got = s.take_saved_session().expect("a Quit & Save session restores");
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].url, "https://a.example/");
        assert!(got[0].active && !got[0].tor, "clearnet active slot round-trips");
        assert_eq!(got[1].width_units, 2);
        assert!(got[1].tor && got[1].url.is_empty(), "Tor slot stays Tor, no URL on disk");

        // One-shot: consumed after the first restore.
        assert!(s.take_saved_session().is_none(), "restore is consumed → default next time");
        // And the rows are purged from disk — no saved URL lingers past the restore.
        assert!(s.load_session_rows().is_empty(), "consumed rows are deleted, not left behind");
    }

    /// A second save wholesale-replaces the first (no stale rows leak across saves).
    #[test]
    fn latest_save_replaces_the_previous_session() {
        let s = Store::open_in_memory();
        s.save_session(&[SessionSlot {
            position: 0, url: "https://old.example/".into(), width_units: 1, active: true, tor: false,
        }]);
        s.save_session(&[
            SessionSlot { position: 0, url: String::new(), width_units: 1, active: false, tor: true },
            SessionSlot { position: 1, url: "https://new.example/".into(), width_units: 1, active: true, tor: false },
        ]);
        let got = s.take_saved_session().expect("restorable");
        assert_eq!(got.len(), 2, "DELETE+INSERT replaces — no leftover row from the old save");
        assert_eq!(got[1].url, "https://new.example/");
    }
}
