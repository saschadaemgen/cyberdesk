//! File logging + in-memory ring buffer (CD-15 HOTFIX, extended CD-18).
//!
//! A windowed release build has no visible stderr, so all diagnostics go to a
//! **rolling daily** log file in the app data dir. One `tracing` subscriber captures
//! BOTH our own lifecycle logs (`tracing::info!` / `debug!` across the shell) AND
//! arti's internal bootstrap / directory-manager events, which is exactly the
//! diagnostic the Tor stall needs.
//!
//! CD-18 adds an in-memory **ring buffer** layer alongside the file layer: the last
//! `RING_CAP` records are kept as structured rows (seq, ts, level, target, msg) so
//! the MF-zone viewer (`cyberdesk://mfzone/`) can stream the log live in the UI over
//! IPC — no tailing of date-suffixed files. Both layers sit under ONE shared
//! `EnvFilter`, so the ring captures exactly what the file does.
//!
//! Never log secrets. The viewer surfaces whatever is logged, so the no-secrets rule
//! matters doubly now: the ring visitor copies only `level`, `target`, and the
//! `message` field — never arbitrary structured key/values.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// The logs directory: `%LOCALAPPDATA%\CyberDesk\logs\` (created if missing).
fn logs_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("APPDATA"))
        .unwrap_or_else(|_| ".".to_string());
    let dir = PathBuf::from(base).join("CyberDesk").join("logs");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// The **actual** current log file: the rolling appender writes
/// `cyberdesk.log.<date>`, so the bare `cyberdesk.log` name never exists on disk
/// (the CD-18 fix for the "file not found" confusion). Returns the newest
/// `cyberdesk.log*` in the logs dir, or the dated-pattern string if none is present
/// yet.
pub fn log_location() -> String {
    let dir = logs_dir();
    let newest = std::fs::read_dir(&dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("cyberdesk.log")
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path());
    match newest {
        Some(p) => p.display().to_string(),
        None => dir.join("cyberdesk.log.<date>").display().to_string(),
    }
}

// --- In-memory ring buffer (CD-18, the MF-zone viewer's data source) --------

/// Kept records. ~2000 lines covers a full Tor bootstrap plus context comfortably.
const RING_CAP: usize = 2000;

/// One captured log record. Owned (no borrows) so it outlives the event.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    /// Monotonic capture sequence — the UI polls incrementally via `since_seq`.
    pub seq: u64,
    /// Epoch milliseconds at capture (the UI formats it client-side).
    pub ts_ms: u64,
    /// `TRACE` | `DEBUG` | `INFO` | `WARN` | `ERROR` (display).
    pub level: &'static str,
    /// Severity rank 0=TRACE..4=ERROR — used for `level_min` filtering so we never
    /// touch tracing's INVERTED `Level: Ord` (ERROR is the lowest `Level`).
    pub sev: u8,
    pub target: String,
    pub msg: String,
}

/// A ring query: keep records newer than `since_seq`, at least `level_min` severity,
/// and whose `target` starts with `target_prefix`. All optional.
#[derive(Default, Clone)]
pub struct LogQuery {
    pub target_prefix: Option<String>,
    pub level_min: Option<u8>,
    pub since_seq: Option<u64>,
}

/// The bounded ring, with its own monotonic sequence counter. Kept small + O(1) per
/// push so the log hot path never stalls.
struct RingInner {
    buf: VecDeque<Record>,
    next_seq: u64,
    cap: usize,
}

impl RingInner {
    fn with_cap(cap: usize) -> Self {
        RingInner {
            buf: VecDeque::with_capacity(cap),
            next_seq: 0,
            cap,
        }
    }

    /// Append a record, evicting the oldest when at capacity. Returns the seq.
    fn push(&mut self, ts_ms: u64, level: &'static str, sev: u8, target: String, msg: String) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        if self.buf.len() >= self.cap {
            self.buf.pop_front();
        }
        self.buf.push_back(Record {
            seq,
            ts_ms,
            level,
            sev,
            target,
            msg,
        });
        seq
    }

    /// Filter oldest→newest into an owned Vec (the UI wants insertion order).
    fn query(&self, q: &LogQuery) -> Vec<Record> {
        self.buf
            .iter()
            .filter(|r| q.since_seq.map_or(true, |s| r.seq > s))
            .filter(|r| q.level_min.map_or(true, |min| r.sev >= min))
            .filter(|r| q.target_prefix.as_deref().map_or(true, |p| r.target.starts_with(p)))
            .cloned()
            .collect()
    }
}

fn ring() -> &'static Mutex<RingInner> {
    static RING: OnceLock<Mutex<RingInner>> = OnceLock::new();
    RING.get_or_init(|| Mutex::new(RingInner::with_cap(RING_CAP)))
}

/// Query the shared ring buffer (recovers from a poisoned lock rather than panic).
pub fn query(q: &LogQuery) -> Vec<Record> {
    ring().lock().unwrap_or_else(|e| e.into_inner()).query(q)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn level_to_sev(level: &tracing::Level) -> u8 {
    match *level {
        tracing::Level::TRACE => 0,
        tracing::Level::DEBUG => 1,
        tracing::Level::INFO => 2,
        tracing::Level::WARN => 3,
        tracing::Level::ERROR => 4,
    }
}

/// Map a `level_min` request word to a severity rank. `None` for an unknown word
/// (the filter then does not constrain severity).
fn sev_from_name(name: &str) -> Option<u8> {
    match name.trim().to_lowercase().as_str() {
        "trace" => Some(0),
        "debug" => Some(1),
        "info" => Some(2),
        "warn" | "warning" => Some(3),
        "error" => Some(4),
        _ => None,
    }
}

/// Extracts only the `message` field off an event — deliberately ignores every other
/// structured field so key/values (which could carry sensitive data) never enter the
/// ring or the viewer.
struct MsgVisitor {
    msg: String,
}

impl tracing::field::Visit for MsgVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.msg.push_str(value);
        }
    }
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && self.msg.is_empty() {
            use std::fmt::Write;
            let _ = write!(self.msg, "{value:?}");
        }
    }
}

/// The ring-buffer tracing layer. Zero-size; all state is in the shared `ring()`.
struct RingLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for RingLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        // Extract everything OUTSIDE the lock (visitor runs on the stack).
        let mut v = MsgVisitor { msg: String::new() };
        event.record(&mut v);
        let meta = event.metadata();
        let level = meta.level();
        let sev = level_to_sev(level);
        let target = meta.target().to_owned();
        let ts_ms = now_ms();
        // Tiny critical section: seq bump + O(1) push. Skip on a poisoned lock — a
        // panic in the log path would be fatal, and never emit a tracing event here
        // (it would re-enter this layer).
        if let Ok(mut g) = ring().lock() {
            g.push(ts_ms, level.as_str(), sev, target, v.msg);
        }
    }
}

/// Build a `LogQuery` from a `get_log_lines` request JSON and serialise the matching
/// ring rows as a JSON array `[{seq,ts,level,target,msg}, ...]` (oldest→newest).
pub fn log_snapshot_json(v: &serde_json::Value) -> String {
    let since_seq = v.get("since_seq").and_then(|s| s.as_u64());
    let (target_prefix, level_min) = match v.get("filter") {
        Some(f) => {
            let tp = f
                .get("target_prefix")
                .and_then(|t| t.as_str())
                .map(|s| s.to_string());
            let lm = f
                .get("level_min")
                .and_then(|l| l.as_str())
                .and_then(sev_from_name);
            (tp, lm)
        }
        None => (None, None),
    };
    let q = LogQuery {
        target_prefix,
        level_min,
        since_seq,
    };
    let rows: Vec<serde_json::Value> = query(&q)
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "seq": r.seq,
                "ts": r.ts_ms,
                "level": r.level,
                "target": r.target,
                "msg": r.msg,
            })
        })
        .collect();
    serde_json::Value::Array(rows).to_string()
}

// --- Subscriber install -----------------------------------------------------

/// The default env-filter (used when `RUST_LOG` is unset). Our lifecycle at debug;
/// arti's crates normally at info (bootstrap milestones + errors). `CYBERDESK_TOR_TRACE`
/// raises the arti/tor crates to **debug** (or **trace** if it is `trace`/`2`) so a
/// stalled bootstrap shows the exact hang point — including the **directory-fetch
/// layer** (`tor_dirclient`), which issues the HTTP-over-Tor consensus request over a
/// built circuit and is the previously-silent step where bootstrap stalls at 15%
/// (CD-15 HOTFIX 2 / HOTFIX 3).
fn default_filter() -> String {
    let arti = match std::env::var("CYBERDESK_TOR_TRACE").ok().as_deref() {
        None | Some("") | Some("0") => "info",
        Some("trace") | Some("2") => "trace",
        _ => "debug", // any other truthy value → debug
    };
    // The arti/tor crate targets that carry the bootstrap detail. `tor_dirmgr` covers
    // its `::state` / `::bootstrap` submodules by prefix. `tor_dirclient` is the
    // directory CLIENT that sends the consensus request over the circuit and reads the
    // response — the silent step in the 15% stall (HOTFIX 3). `tor_memquota` catches a
    // memory-quota reservation that could gate the fetch.
    let tor_targets = [
        "arti_client",
        "tor_dirmgr",
        "tor_dirclient",
        "tor_guardmgr",
        "tor_chanmgr",
        "tor_proto",
        "tor_circmgr",
        "tor_netdir",
        "tor_netdoc",
        "tor_memquota",
    ];
    let mut f = String::from("info,cyberdesk=debug");
    for t in tor_targets {
        f.push_str(&format!(",{t}={arti}"));
    }
    f
}

/// Route panics through `tracing` so a swallowed panic (e.g. inside a tokio-spawned
/// arti task, which tokio catches and would otherwise print only to a non-existent
/// console) is captured in the log file (CD-15 HOTFIX 2). Installed once.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| info.payload().downcast_ref::<String>().map(|s| s.as_str()))
            .unwrap_or("<non-string panic payload>");
        tracing::error!(location = %loc, "PANIC: {msg}");
        prev(info);
    }));
}

/// Install the file + ring subscriber once (browser process only, before anything
/// logs). The non-blocking writer's `WorkerGuard` is kept for the process lifetime so
/// buffered lines are flushed.
pub fn init() {
    static GUARD: OnceLock<WorkerGuard> = OnceLock::new();
    if GUARD.get().is_some() {
        return;
    }
    let dir = logs_dir();
    let appender = tracing_appender::rolling::daily(&dir, "cyberdesk.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_filter()));

    // Ensure the ring exists before the subscriber starts feeding it.
    let _ = ring();

    // ONE shared EnvFilter gates the whole subscriber, so the file and the ring see
    // exactly the same event set (the ring never captures less than the file does).
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true);

    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(RingLayer)
        .try_init()
        .is_ok();

    if installed {
        let _ = GUARD.set(guard);
        install_panic_hook();
        tracing::info!(
            location = %log_location(),
            tor_trace = std::env::var("CYBERDESK_TOR_TRACE").is_ok(),
            "logging initialised (rolling daily cyberdesk.log + ring buffer)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(inner: &mut RingInner, sev: u8, target: &str, msg: &str) -> u64 {
        let level = match sev {
            0 => "TRACE",
            1 => "DEBUG",
            2 => "INFO",
            3 => "WARN",
            _ => "ERROR",
        };
        inner.push(1000, level, sev, target.to_string(), msg.to_string())
    }

    #[test]
    fn seq_is_monotonic_and_capacity_evicts_oldest() {
        let mut ring = RingInner::with_cap(3);
        assert_eq!(rec(&mut ring, 2, "cyberdesk", "a"), 0);
        assert_eq!(rec(&mut ring, 2, "cyberdesk", "b"), 1);
        assert_eq!(rec(&mut ring, 2, "cyberdesk", "c"), 2);
        // Over capacity: oldest ("a", seq 0) is evicted, seq keeps climbing.
        assert_eq!(rec(&mut ring, 2, "cyberdesk", "d"), 3);
        let all = ring.query(&LogQuery::default());
        assert_eq!(all.len(), 3);
        assert_eq!(all.iter().map(|r| r.msg.as_str()).collect::<Vec<_>>(), ["b", "c", "d"]);
        assert_eq!(all[0].seq, 1); // "a"/seq 0 gone
    }

    #[test]
    fn since_seq_returns_only_newer_records() {
        let mut ring = RingInner::with_cap(10);
        for i in 0..5 {
            rec(&mut ring, 2, "cyberdesk", &format!("m{i}"));
        }
        let q = LogQuery {
            since_seq: Some(2),
            ..Default::default()
        };
        let got = ring.query(&q);
        assert_eq!(got.iter().map(|r| r.seq).collect::<Vec<_>>(), [3, 4]);
    }

    #[test]
    fn target_prefix_filters() {
        let mut ring = RingInner::with_cap(10);
        rec(&mut ring, 2, "cyberdesk::tor", "socks bind");
        rec(&mut ring, 2, "tor_dirmgr::bootstrap", "consensus");
        rec(&mut ring, 2, "cyberdesk::app", "frame");
        // "tor"-ish targets: our tor module + arti's tor_* crates. Prefix "tor" hits
        // arti; prefix "cyberdesk::tor" hits ours. Confirm each narrows correctly.
        let arti = ring.query(&LogQuery {
            target_prefix: Some("tor_".to_string()),
            ..Default::default()
        });
        assert_eq!(arti.len(), 1);
        assert_eq!(arti[0].target, "tor_dirmgr::bootstrap");
        let ours = ring.query(&LogQuery {
            target_prefix: Some("cyberdesk::tor".to_string()),
            ..Default::default()
        });
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].msg, "socks bind");
    }

    #[test]
    fn level_min_uses_severity_rank_not_inverted_level_ord() {
        let mut ring = RingInner::with_cap(10);
        rec(&mut ring, 0, "x", "trace");
        rec(&mut ring, 1, "x", "debug");
        rec(&mut ring, 2, "x", "info");
        rec(&mut ring, 3, "x", "warn");
        rec(&mut ring, 4, "x", "error");
        // level_min = INFO (sev 2) keeps info/warn/error (sev >= 2).
        let q = LogQuery {
            level_min: Some(2),
            ..Default::default()
        };
        let got = ring.query(&q);
        assert_eq!(got.iter().map(|r| r.msg.as_str()).collect::<Vec<_>>(), ["info", "warn", "error"]);
    }

    #[test]
    fn sev_name_mapping() {
        assert_eq!(sev_from_name("info"), Some(2));
        assert_eq!(sev_from_name("DEBUG"), Some(1));
        assert_eq!(sev_from_name("warning"), Some(3));
        assert_eq!(sev_from_name("nonsense"), None);
    }
}
