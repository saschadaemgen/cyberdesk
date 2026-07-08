//! Persistent application state (SQLite via rusqlite).
//!
//! A schema-versioned `settings` key/value table plus a `meta` table holding the
//! selected `template` (only value: "cyber"). Lives in the OS app-data
//! directory, never in the repo.

// The full store API is defined in Stage A; the get/set/all_settings/template
// methods are consumed by the settings IPC in Stage D.
#![allow(dead_code)]

use std::path::PathBuf;

use rusqlite::Connection;

const SCHEMA_VERSION: i64 = 1;

pub struct Store {
    conn: Connection,
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
        let path = data_dir().join("state.db");
        let conn = Connection::open(&path).expect("failed to open state.db");
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
        if let Ok(mut stmt) = self.conn.prepare("SELECT key, value FROM settings") {
            if let Ok(rows) = stmt.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            }) {
                out.extend(rows.filter_map(Result::ok));
            }
        }
        out
    }
}
