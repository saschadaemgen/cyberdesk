//! Embedded Tor engine (CD-15, D-0026) — pure-Rust Tor via `arti-client` on a
//! background tokio runtime, exposing a **per-slot** local SOCKS5 endpoint that
//! each Tor CEF request context proxies through. Each slot id has its own loopback
//! port bound to its own *isolated* [`TorClient`], so two Tor windows never share a
//! circuit (per-window circuit isolation). Bootstrap runs OFF the shell thread and
//! status is a lock-free atomic, so the UI never blocks on Tor. This is NetGuard's
//! second sanctioned outbound path (D-0004 → D-0026): user-driven browsing.
//!
//! Leak stance (host side of the checklist): the SOCKS relay does **remote DNS** —
//! a hostname (SOCKS ATYP=domain) is handed to arti unresolved, never resolved
//! locally, so DNS goes through Tor. The WebRTC / QUIC / proxy-bypass half of the
//! checklist is enforced on the CEF request context (Stage B). Empirical
//! verification (check.torproject.org, DNS-leak, WebRTC) is Sascha's live run.
//!
//! Residual risk (documented, D-0026): embedded arti may `process::exit(1)` on an
//! obsolete consensus, which would take the shell down; the subprocess integration
//! would isolate that. Embedded was chosen (single-binary doctrine, SimpleGoX
//! precedent) with this risk accepted and noted.

// Parts of this API (socks_port, per-slot wiring) are consumed by CD-15 Stage B;
// keep it complete meanwhile.
#![allow(dead_code)]

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use arti_client::config::CfgPath;
use arti_client::{TorAddr, TorClient, TorClientConfig};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::compat::FuturesAsyncReadCompatExt;

use crate::slots::MAX_SLOTS;

/// A bootstrapped Tor client on arti's preferred (tokio) runtime.
type Client = TorClient<tor_rtcompat::PreferredRuntime>;

/// Engine status (lock-free, read by the UI / Tor glyph).
pub const STATUS_IDLE: u8 = 0; // never started (Tor engine off / not yet used)
pub const STATUS_BOOTSTRAPPING: u8 = 1;
pub const STATUS_READY: u8 = 2;
pub const STATUS_FAILED: u8 = 3;

static STATUS: AtomicU8 = AtomicU8::new(STATUS_IDLE);

/// "New identity" epoch (CD-18). arti-client 0.44 exposes no single global
/// new-identity call, but our per-slot SOCKS relays each hold an *isolated* client;
/// bumping this epoch makes every listener drop and re-create its isolated client on
/// its next connection, so subsequent streams ride FRESH circuits under a fresh
/// isolation group (the user reloads a page to use its new circuit). Cheap + safe:
/// it only changes when a new isolated client is built — never the proxy or the
/// fail-closed guarantee.
static NEW_IDENTITY_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Request fresh Tor circuits / identity for subsequent connections (CD-18). Just a
/// lock-free epoch bump, so it is safe to call from any thread (e.g. the CEF UI
/// thread handling the settings button).
pub fn new_identity() {
    let epoch = NEW_IDENTITY_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    tracing::info!(epoch, "tor: new identity requested (fresh circuits for new streams)");
}

/// Hard cap on the first bootstrap (CD-15 HOTFIX): a Tor-blocking network or a bad
/// cache dir surfaces as `Failed` instead of infinite "connecting". Overridable via
/// `CYBERDESK_TOR_BOOTSTRAP_SECS` (a very slow Tor network may need longer; tests
/// use a small value to exercise the failure path). Default 90 s.
fn bootstrap_timeout() -> std::time::Duration {
    let secs = std::env::var("CYBERDESK_TOR_BOOTSTRAP_SECS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .unwrap_or(90);
    std::time::Duration::from_secs(secs)
}

/// The reason for `STATUS_FAILED`, surfaced in the UI. Empty unless failed.
fn failed_reason() -> &'static Mutex<String> {
    static R: OnceLock<Mutex<String>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(String::new()))
}

/// Record a failure: set `STATUS_FAILED` + the reason, and log it.
fn set_failed(reason: &str) {
    tracing::error!(reason, "tor engine FAILED");
    *failed_reason().lock().unwrap() = reason.to_string();
    STATUS.store(STATUS_FAILED, Ordering::SeqCst);
}

/// The current failure reason (empty unless `status() == STATUS_FAILED`).
pub fn fail_reason() -> String {
    failed_reason().lock().unwrap().clone()
}

/// Build the arti config with an **explicit, known-writable** state + cache dir
/// under our app data dir (CD-15 HOTFIX). The default config uses `${ARTI_*}` path
/// variables that must resolve at runtime; if they don't (or the dir isn't
/// writable) bootstrap stalls — a literal path we create ourselves avoids that.
fn tor_config() -> Result<TorClientConfig, String> {
    let base = std::env::var("LOCALAPPDATA")
        .or_else(|_| std::env::var("APPDATA"))
        .unwrap_or_else(|_| ".".to_string());
    let root = std::path::PathBuf::from(base).join("CyberDesk").join("tor");
    let state = root.join("state");
    let cache = root.join("cache");
    std::fs::create_dir_all(&state).map_err(|e| format!("mkdir {}: {e}", state.display()))?;
    std::fs::create_dir_all(&cache).map_err(|e| format!("mkdir {}: {e}", cache.display()))?;
    tracing::info!(state = %state.display(), cache = %cache.display(), "tor state/cache dirs ready");
    let mut b = TorClientConfig::builder();
    b.storage()
        .state_dir(CfgPath::new_literal(state))
        .cache_dir(CfgPath::new_literal(cache));
    b.build().map_err(|e| format!("config build: {e}"))
}

/// The base loopback SOCKS port; slot id `i` listens on `SOCKS_BASE_PORT + i`.
/// Loopback only (127.0.0.1) — never a public bind.
const SOCKS_BASE_PORT: u16 = 9250;

/// The bootstrapped base client, shared with the per-slot SOCKS listeners once
/// ready. `None` until bootstrap succeeds (or forever, if it fails).
fn base_client() -> &'static Mutex<Option<Arc<Client>>> {
    static C: OnceLock<Mutex<Option<Arc<Client>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Current engine status.
pub fn status() -> u8 {
    STATUS.load(Ordering::Relaxed)
}

/// The loopback SOCKS5 port a Tor context for slot `id` proxies through.
pub fn socks_port(id: usize) -> u16 {
    SOCKS_BASE_PORT + (id.min(MAX_SLOTS - 1) as u16)
}

/// Start the Tor engine once: a background tokio runtime that binds the per-slot
/// SOCKS listeners immediately, then bootstraps arti (so a slot toggled to Tor
/// while still connecting has a live port to retry against). Idempotent — a second
/// call while already started is a no-op. Never blocks the caller.
pub fn init() {
    if STATUS
        .compare_exchange(
            STATUS_IDLE,
            STATUS_BOOTSTRAPPING,
            Ordering::SeqCst,
            Ordering::SeqCst,
        )
        .is_err()
    {
        tracing::debug!("tor::init called but engine already started");
        return;
    }
    tracing::info!("tor::init — spawning the Tor engine thread");
    match std::thread::Builder::new()
        .name("tor-engine".to_string())
        .spawn(run)
    {
        Ok(_) => {}
        Err(e) => {
            set_failed(&format!("could not spawn tor thread: {e}"));
        }
    }
}

fn run() {
    tracing::info!("tor-engine thread: building the tokio runtime");
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            set_failed(&format!("tokio runtime build failed: {e}"));
            return;
        }
    };
    tracing::info!("tor-engine: runtime built, entering block_on");

    rt.block_on(async {
        // Bind the per-slot SOCKS listeners up front (ports live during bootstrap);
        // each waits for the base client to be ready before it will relay.
        for id in 0..MAX_SLOTS {
            tokio::spawn(socks_listener(socks_port(id)));
        }

        // Bootstrap off the shell thread, with a hard timeout so a Tor-blocking
        // network / bad cache dir surfaces as FAILED instead of infinite connecting.
        let config = match tor_config() {
            Ok(c) => c,
            Err(e) => {
                set_failed(&format!("config/state-dir error: {e}"));
                std::future::pending::<()>().await;
                return;
            }
        };
        let timeout = bootstrap_timeout();
        tracing::info!(timeout_s = timeout.as_secs(), "tor bootstrap: begin");
        let boot = tokio::time::timeout(timeout, TorClient::create_bootstrapped(config));
        match boot.await {
            Ok(Ok(client)) => {
                *base_client().lock().unwrap() = Some(client);
                STATUS.store(STATUS_READY, Ordering::SeqCst);
                tracing::info!("tor bootstrap: READY");
            }
            Ok(Err(e)) => set_failed(&format!("bootstrap error: {e}")),
            Err(_elapsed) => set_failed(&format!(
                "bootstrap timed out after {}s (network blocking Tor?)",
                timeout.as_secs()
            )),
        }

        // Keep the runtime (and the SOCKS listeners) alive for the process life.
        std::future::pending::<()>().await;
    });
}

/// One per-slot SOCKS5 listener. Its own *isolated* client (created lazily once the
/// base is ready) puts this slot's streams on their own circuit family.
async fn socks_listener(port: u16) {
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await {
        Ok(l) => {
            tracing::info!(port, "tor SOCKS listener bound (127.0.0.1)");
            l
        }
        Err(e) => {
            tracing::error!(port, error = %e, "tor SOCKS bind FAILED");
            return;
        }
    };
    // The isolated client for THIS slot, created once the base client is ready and
    // reused for every connection (stable per-slot circuit isolation). It is
    // re-created when the "new identity" epoch advances (CD-18) so new streams ride
    // fresh circuits under a fresh isolation group.
    let mut client: Option<Arc<Client>> = None;
    let mut client_epoch = NEW_IDENTITY_EPOCH.load(Ordering::SeqCst);

    loop {
        let (sock, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(_) => continue,
        };
        let epoch = NEW_IDENTITY_EPOCH.load(Ordering::SeqCst);
        if client.is_none() || epoch != client_epoch {
            client = base_client().lock().unwrap().as_ref().map(|c| c.isolated_client());
            client_epoch = epoch;
        }
        let Some(c) = client.clone() else {
            // Not bootstrapped yet (or failed): drop the connection; the browser
            // shows a connecting/error state and retries once Tor is ready.
            drop(sock);
            continue;
        };
        tokio::spawn(handle_socks(c, sock));
    }
}

/// Handle one SOCKS5 CONNECT: no-auth handshake, parse the target (IPv4 / IPv6 /
/// **domain — resolved remotely through Tor, never locally**), open the Tor
/// stream, reply, and relay bytes both ways.
async fn handle_socks(client: Arc<Client>, mut sock: TcpStream) {
    if socks_connect(&client, &mut sock).await.is_err() {
        let _ = sock.shutdown().await;
    }
}

async fn socks_connect(client: &Arc<Client>, sock: &mut TcpStream) -> std::io::Result<()> {
    // --- Greeting: VER, NMETHODS, METHODS[] -> reply no-auth (0x00). ---
    let mut head = [0u8; 2];
    sock.read_exact(&mut head).await?;
    if head[0] != 0x05 {
        return Err(bad("not SOCKS5"));
    }
    let nmethods = head[1] as usize;
    let mut methods = vec![0u8; nmethods];
    sock.read_exact(&mut methods).await?;
    sock.write_all(&[0x05, 0x00]).await?; // no authentication required

    // --- Request: VER, CMD, RSV, ATYP, ADDR, PORT ---
    let mut req = [0u8; 4];
    sock.read_exact(&mut req).await?;
    if req[0] != 0x05 || req[1] != 0x01 {
        // Only CONNECT is supported; reply "command not supported".
        reply(sock, 0x07).await?;
        return Err(bad("unsupported SOCKS command"));
    }

    // The target that gets handed to Tor: a hostname (remote DNS) or an IP.
    enum Target {
        Host(String),
        Addr(IpAddr),
    }
    let target = match req[3] {
        0x01 => {
            let mut b = [0u8; 4];
            sock.read_exact(&mut b).await?;
            Target::Addr(IpAddr::from(b))
        }
        0x04 => {
            let mut b = [0u8; 16];
            sock.read_exact(&mut b).await?;
            Target::Addr(IpAddr::from(b))
        }
        0x03 => {
            let mut len = [0u8; 1];
            sock.read_exact(&mut len).await?;
            let mut name = vec![0u8; len[0] as usize];
            sock.read_exact(&mut name).await?;
            let host = String::from_utf8(name).map_err(|_| bad("bad host"))?;
            Target::Host(host)
        }
        _ => {
            reply(sock, 0x08).await?; // address type not supported
            return Err(bad("bad ATYP"));
        }
    };
    let mut port_bytes = [0u8; 2];
    sock.read_exact(&mut port_bytes).await?;
    let port = u16::from_be_bytes(port_bytes);

    // --- Open the Tor stream. A hostname is handed to arti unresolved (remote DNS
    // through Tor, never a local resolver). An explicit IP (SOCKS ATYP=1/4) came
    // straight from the client, not a local resolution, so it is connected via
    // `dangerously_from` — intentional here, and the only place IPs enter. ---
    let stream = match target {
        Target::Host(h) => client.connect((h.as_str(), port)).await,
        Target::Addr(a) => match TorAddr::dangerously_from((a, port)) {
            Ok(ta) => client.connect(ta).await,
            Err(_) => {
                reply(sock, 0x08).await?;
                return Err(bad("bad addr"));
            }
        },
    };
    let data = match stream {
        Ok(d) => d,
        Err(_) => {
            reply(sock, 0x05).await?; // connection refused
            return Err(bad("tor connect failed"));
        }
    };

    // Success: bound-addr fields are unused by the client, send zeros.
    reply(sock, 0x00).await?;

    // Relay both directions until either side closes. `data` is arti's futures-based
    // DataStream, bridged to tokio via the compat wrapper.
    let mut data = data.compat();
    let _ = tokio::io::copy_bidirectional(sock, &mut data).await;
    Ok(())
}

/// Send a SOCKS5 reply with the given status byte and a zeroed bound address.
async fn reply(sock: &mut TcpStream, status: u8) -> std::io::Result<()> {
    sock.write_all(&[0x05, status, 0x00, 0x01, 0, 0, 0, 0, 0, 0]).await
}

fn bad(msg: &str) -> std::io::Error {
    std::io::Error::other(msg.to_string())
}
