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

const SCHEMA_VERSION: i64 = 5;

/// History is capped at this many rows; the oldest are pruned on each insert
/// (D-0014). Local only — no sync, no export.
const HISTORY_CAP: i64 = 10_000;

/// One command-palette suggestion row: a favorite or a history entry.
pub struct Suggestion {
    pub url: String,
    pub title: String,
    pub favorite: bool,
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
        store.migrate();
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
        self.conn
            .pragma_update(None, "user_version", SCHEMA_VERSION)
            .ok();
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
        // CD-07: the command-bar search-engine choice, default Google.
        self.set_if_absent("search_engine", "google");
        // CD-15: the Tor engine is available by default; new windows are clearnet.
        self.set_if_absent("tor_enabled", "true");
        self.set_if_absent("tor_default", "false");
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
}
