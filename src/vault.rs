//! CyberDesk Vault — Stage 1 crypto core (CD-40, D-0058).
//!
//! Envelope key management: one random 256-bit **Vault Master Key (VMK)**
//! protects the vault's sensitive data; the VMK itself is never derived from
//! any single factor. Multiple independent **envelopes** each wrap the VMK —
//! enrolling or removing an unlock method re-wraps the VMK, never re-encrypts
//! the protected data.
//!
//! ## Enrollment methods (Stage 1)
//!
//! * **Passphrase** — Argon2id (explicit tuned params, stored per method) →
//!   32-byte wrapping key. The root/bootstrap method; always present.
//! * **Recovery key** — 32 random bytes, displayed once at mint time in a
//!   grouped Crockford-base32 format with a typo-detecting checksum. The
//!   mandatory non-hardware fallback; always present.
//! * **Passkey (WebAuthn PRF)** — the PRF-derived secret is the wrapping key.
//!   The core treats it as an opaque 32-byte method secret; the WebAuthn layer
//!   lands in the final CD-40 sub-stage.
//!
//! ## Unlock policy is structural, not a checked flag
//!
//! The policy "N methods must be presented together" is enforced by **which
//! envelopes exist**: at `required = 1` every method has its own envelope; at
//! `required = 2` only *pairs* of methods have envelopes, each wrapped by a
//! key combined (BLAKE2s, domain-separated) from both methods' wrapping keys.
//! No single factor can open any envelope of a 2-required vault — editing the
//! `required` field in `vault.json` changes nothing, because the mutable field
//! is UI metadata; the cryptography is in the envelope set. The design
//! generalizes to any N ≤ enrolled methods (k-combinations).
//!
//! ## Escrows make re-wrapping possible from an unlocked session
//!
//! Every method's wrapping key is also stored wrapped **under the VMK** (an
//! "escrow"). Enrolling a passkey / changing the policy from an unlocked
//! session needs every method's wrapping key to build the new combination
//! envelopes — the escrows provide them without re-prompting for each factor.
//! This adds nothing an attacker could use: whoever holds the VMK has already
//! won the current vault (the escrows are decryptable only *with* the VMK),
//! and rotating a method replaces its escrow. Recorded in D-0058.
//!
//! ## Never-brick rule
//!
//! There must always be at least one way to unlock that requires no hardware:
//! the passphrase and the recovery key are always enrolled and cannot be
//! removed (Stage 1), and every envelope set must contain at least one
//! envelope whose methods are all non-hardware. [`VaultFile::load_from`]
//! refuses a file violating the invariant, so a hand-edited or corrupted
//! vault fails closed instead of booby-trapping a later unlock.
//!
//! ## Memory hygiene (closes the CD-33-deferred Tasks C/D for vault keys)
//!
//! All key material lives in [`SecretBuf`]s: dedicated `VirtualAlloc`ed pages,
//! `VirtualLock`ed out of the pagefile, zeroized then unlocked and released on
//! drop. Allocation **fails closed** — if the pages cannot be locked (after
//! one working-set bump), no key material is produced at all. AEAD runs
//! through the `*_in_place_detached` APIs so plaintext keys never transit an
//! allocation the cipher crate owns; Argon2 runs with an explicitly provided
//! block matrix that is zeroized after derivation. Bounded residual (internal
//! doc honesty, D-0044): transient stack copies inside the crypto crates and
//! the unlocked Argon2 matrix *during* derivation are not lockable from this
//! layer; hibernation (`hiberfil.sys`) snapshots even locked pages.
//!
//! No custom cryptography: Argon2id (`argon2`), XChaCha20-Poly1305
//! (`chacha20poly1305`), BLAKE2s (`blake2`), OS CSPRNG (`getrandom`), volatile
//! erasure (`zeroize`) — all pinned, license-checked (D-0005), verified at
//! source against the exact crate versions.

// Stage 1a ships the crypto core + persistence; the boot gate (1b), the
// config/tile surface (1c) and the WebAuthn PRF layer (1d) consume this API in
// the following sub-stages. Mirrors the store.rs precedent.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use argon2::{Algorithm, Argon2, Block, Params, Version};
use blake2::{Blake2s256, Digest};
use chacha20poly1305::{
    Key, Tag, XChaCha20Poly1305, XNonce,
    aead::{AeadInPlace, KeyInit},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Key sizes. Everything is 256-bit: the VMK, every wrapping key, the recovery
/// key, and the combined envelope keys.
pub const KEY_LEN: usize = 32;
/// XChaCha20-Poly1305 extended nonce (24 bytes — safe to draw at random per
/// wrap; the birthday bound on 192 bits is unreachable).
const NONCE_LEN: usize = 24;
/// Poly1305 authentication tag.
const TAG_LEN: usize = 16;
/// Per-passphrase-envelope Argon2 salt (16 random bytes; crate minimum is 8).
const SALT_LEN: usize = 16;
/// Checksum bytes appended to the recovery key before display encoding —
/// catches typos locally (2^-24 miss chance) before any unlock attempt runs.
const RECOVERY_CHECK_LEN: usize = 3;
/// Minimum passphrase length in bytes (the UI may be stricter, never weaker).
pub const MIN_PASSPHRASE_LEN: usize = 8;
/// The vault file format version this build reads and writes.
const VAULT_VERSION: u32 = 1;

/// AEAD domain separation. Every wrap context has its own associated data, so
/// a blob can never be replayed in a different role (an envelope is not an
/// escrow is not a sealed-state blob), and an envelope is bound to the exact
/// method set that keys it.
const AAD_ENVELOPE: &str = "cyberdesk.vault.v1.envelope:";
const AAD_ESCROW: &str = "cyberdesk.vault.v1.escrow:";
/// Sealed app-state container magic — also its AAD.
const SEAL_MAGIC: &[u8; 8] = b"CDSEAL01";
/// Domain prefix for combining method wrapping keys into an envelope key.
/// Single-method envelopes run through the same PRF (uniform code path and
/// domain separation of raw method secrets from envelope keys).
const COMBINE_DOMAIN: &[u8] = b"cyberdesk.vault.v1.combine";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Vault errors. `Crypto` is deliberately uniform — a wrong passphrase, a
/// wrong recovery key and a tampered envelope are indistinguishable to the
/// caller (no oracle; the attacker holding `vault.json` learns nothing from
/// the failure mode).
#[derive(Debug, PartialEq, Eq)]
pub enum VaultError {
    /// Secure-memory allocation or locking failed (fail-closed: no key
    /// material is ever produced in unlockable pages).
    Mem(&'static str),
    /// Argon2 parameter or derivation failure.
    Kdf(String),
    /// Unlock/unwrap failed: wrong factor or tampered data. Uniform.
    Crypto,
    /// Malformed input (recovery-key format, hex, file shape, unknown ids).
    Format(String),
    /// A policy/invariant violation (never-brick, required out of range).
    Policy(String),
    /// Filesystem error reading or writing the vault files.
    Io(String),
}

impl std::fmt::Display for VaultError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultError::Mem(m) => write!(f, "secure memory unavailable: {m}"),
            VaultError::Kdf(m) => write!(f, "key derivation failed: {m}"),
            VaultError::Crypto => write!(f, "unlock failed"),
            VaultError::Format(m) => write!(f, "malformed vault data: {m}"),
            VaultError::Policy(m) => write!(f, "vault policy violation: {m}"),
            VaultError::Io(m) => write!(f, "vault i/o failed: {m}"),
        }
    }
}

type Result<T> = std::result::Result<T, VaultError>;

// ---------------------------------------------------------------------------
// SecretBuf — locked, zeroized key memory
// ---------------------------------------------------------------------------

/// Owner of key material: dedicated whole pages (`VirtualAlloc`), locked out
/// of the pagefile (`VirtualLock`), zeroized then unlocked and released on
/// drop. Dedicated pages matter: `VirtualUnlock` unlocks *pages*, so secrets
/// must never share a page with an allocator neighbor whose drop would unlock
/// them; and a page-owning buffer can never be silently memcpy'd by a
/// reallocation. Fail-closed: if the pages cannot be locked even after one
/// working-set bump, construction fails and no key bytes are ever written.
#[cfg(windows)]
pub struct SecretBuf {
    ptr: std::ptr::NonNull<u8>,
    len: usize,
}

// Sole owner of its pages; the unlock worker thread (Stage 1b) hands the VMK
// back to the main thread.
#[cfg(windows)]
unsafe impl Send for SecretBuf {}

#[cfg(windows)]
mod winmem {
    use core::ffi::c_void;

    // House style (see app.rs GetTimeZoneInformation): direct kernel32 externs,
    // no windows-crate dependency. kernel32 is in Rust's default MSVC link set.
    unsafe extern "system" {
        pub fn VirtualAlloc(
            addr: *mut c_void,
            size: usize,
            alloc_type: u32,
            protect: u32,
        ) -> *mut c_void;
        pub fn VirtualFree(addr: *mut c_void, size: usize, free_type: u32) -> i32;
        pub fn VirtualLock(addr: *mut c_void, size: usize) -> i32;
        pub fn VirtualUnlock(addr: *mut c_void, size: usize) -> i32;
        pub fn GetCurrentProcess() -> isize;
        pub fn GetProcessWorkingSetSize(process: isize, min: *mut usize, max: *mut usize) -> i32;
        pub fn SetProcessWorkingSetSize(process: isize, min: usize, max: usize) -> i32;
    }

    pub const MEM_COMMIT: u32 = 0x1000;
    pub const MEM_RESERVE: u32 = 0x2000;
    pub const MEM_RELEASE: u32 = 0x8000;
    pub const PAGE_READWRITE: u32 = 0x04;
    pub const PAGE_SIZE: usize = 4096;
    /// Working-set headroom added when the first `VirtualLock` is refused: the
    /// requested size plus margin, so a burst of transient wrapping keys
    /// during one unlock never trips the default minimum again.
    pub const WS_MARGIN: usize = 4 * 1024 * 1024;
}

#[cfg(windows)]
impl SecretBuf {
    /// Allocate `len` zeroed, page-locked bytes. Fails closed on any
    /// allocation or locking error.
    pub fn zeroed(len: usize) -> Result<Self> {
        use winmem::*;
        assert!(len > 0, "SecretBuf must not be empty");
        let size = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        // VirtualAlloc'd pages arrive zeroed by the OS.
        let ptr = unsafe {
            VirtualAlloc(
                std::ptr::null_mut(),
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        let Some(ptr) = std::ptr::NonNull::new(ptr.cast::<u8>()) else {
            return Err(VaultError::Mem("VirtualAlloc failed"));
        };
        let mut locked = unsafe { VirtualLock(ptr.as_ptr().cast(), size) } != 0;
        if !locked {
            // The default process minimum working set (~200 KiB of lockable
            // pages) can be exhausted; grow it once by the request + margin
            // and retry. Still failing → release and refuse (fail-closed).
            let proc = unsafe { GetCurrentProcess() };
            let (mut min, mut max) = (0usize, 0usize);
            if unsafe { GetProcessWorkingSetSize(proc, &mut min, &mut max) } != 0 {
                let grow = size + WS_MARGIN;
                unsafe { SetProcessWorkingSetSize(proc, min + grow, max + grow) };
                locked = unsafe { VirtualLock(ptr.as_ptr().cast(), size) } != 0;
            }
        }
        if !locked {
            unsafe { VirtualFree(ptr.as_ptr().cast(), 0, MEM_RELEASE) };
            return Err(VaultError::Mem("VirtualLock refused (working set)"));
        }
        Ok(Self { ptr, len })
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Volatile-zero the contents in place (also runs on drop).
    pub fn wipe(&mut self) {
        self.as_mut_slice().zeroize();
    }
}

#[cfg(windows)]
impl Drop for SecretBuf {
    fn drop(&mut self) {
        use winmem::*;
        // Zeroize while the pages are still locked, then unlock and release.
        self.wipe();
        let size = self.len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        unsafe {
            VirtualUnlock(self.ptr.as_ptr().cast(), size);
            VirtualFree(self.ptr.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}

/// Non-Windows fallback (dev/CI portability only — the product target is
/// Windows, D-0001): zeroize-on-drop without a page lock.
#[cfg(not(windows))]
pub struct SecretBuf {
    buf: Vec<u8>,
}

#[cfg(not(windows))]
impl SecretBuf {
    pub fn zeroed(len: usize) -> Result<Self> {
        assert!(len > 0, "SecretBuf must not be empty");
        Ok(Self { buf: vec![0u8; len] })
    }
    pub fn as_slice(&self) -> &[u8] {
        &self.buf
    }
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.buf
    }
    pub fn len(&self) -> usize {
        self.buf.len()
    }
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
    pub fn wipe(&mut self) {
        self.buf.zeroize();
    }
}

#[cfg(not(windows))]
impl Drop for SecretBuf {
    fn drop(&mut self) {
        self.buf.zeroize();
    }
}

impl SecretBuf {
    /// A locked copy of `bytes`. The source's hygiene is the caller's business
    /// (used for staging public ciphertext into a decrypt-in-place buffer, and
    /// by tests).
    pub fn copy_of(bytes: &[u8]) -> Result<Self> {
        let mut b = Self::zeroed(bytes.len())?;
        b.as_mut_slice().copy_from_slice(bytes);
        Ok(b)
    }

    /// A fresh CSPRNG-filled locked buffer (VMK, recovery key, salts live
    /// elsewhere — this is for keys).
    pub fn random(len: usize) -> Result<Self> {
        let mut b = Self::zeroed(len)?;
        getrandom::fill(b.as_mut_slice())
            .map_err(|e| VaultError::Kdf(format!("csprng failed: {e}")))?;
        Ok(b)
    }
}

/// Redacted — key material must never reach a log line.
impl std::fmt::Debug for SecretBuf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SecretBuf({} bytes, redacted)", self.len())
    }
}

// ---------------------------------------------------------------------------
// Small helpers: hex + CSPRNG
// ---------------------------------------------------------------------------

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn hex_decode(s: &str) -> Result<Vec<u8>> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(VaultError::Format("bad hex".into()));
    }
    Ok((0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect())
}

fn rand_array<const N: usize>() -> Result<[u8; N]> {
    let mut a = [0u8; N];
    getrandom::fill(&mut a).map_err(|e| VaultError::Kdf(format!("csprng failed: {e}")))?;
    Ok(a)
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// Argon2id cost parameters, stored per passphrase method so they can be
/// tuned/migrated later without breaking older envelopes. The product default
/// is RFC 9106's second recommended configuration (64 MiB, t=3, p=4) — the
/// memory-constrained profile; the pure-Rust `argon2` crate computes lanes
/// sequentially, which changes wall-clock, never the output or the security
/// parameters. Exposed as settings in the config surface (CD-40 Task 7).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
}

impl KdfParams {
    /// RFC 9106 second recommendation: m=64 MiB, t=3, p=4.
    pub const PRODUCT: KdfParams = KdfParams { m_cost_kib: 64 * 1024, t_cost: 3, p_cost: 4 };
}

/// What kind of factor a method is. `hardware()` feeds the never-brick rule:
/// losing every hardware token must never brick the vault.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodKind {
    Passphrase,
    RecoveryKey,
    Passkey,
}

impl MethodKind {
    pub fn hardware(self) -> bool {
        matches!(self, MethodKind::Passkey)
    }
}

/// One enrolled unlock method. The wrapping key itself is never stored here —
/// it is re-derived at unlock (passphrase), re-presented (recovery key,
/// passkey PRF), or recovered from its escrow (mutations from an unlocked
/// session).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Method {
    /// Stable id: `"passphrase"`, `"recovery"`, `"passkey-<hex>"`. Envelope
    /// membership and escrows reference methods by id.
    pub id: String,
    pub kind: MethodKind,
    /// User-facing label for the config surface ("Passphrase", "YubiKey 5"…).
    pub label: String,
    /// Mint time (unix epoch ms) for honest status display.
    pub created_ms: u64,
    /// Passphrase methods only: the Argon2id cost parameters…
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<KdfParams>,
    /// …and the per-method random salt (hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
}

/// One VMK envelope: the VMK wrapped by the combined key of `method_ids`
/// (sorted; 1 id at `required=1`, 2 at `required=2`, …). `wrapped` is
/// ciphertext ‖ tag, hex; the AAD binds the exact method set.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub method_ids: Vec<String>,
    pub nonce: String,
    pub wrapped: String,
}

/// One method's wrapping key, wrapped under the VMK (see module docs on why
/// escrows exist and why they add no attack surface).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Escrow {
    pub method_id: String,
    pub nonce: String,
    pub wrapped: String,
}

/// The persisted vault metadata (`vault.json`). Contains no secret an
/// attacker could use without a factor: salts, nonces and AEAD blobs only.
/// `required` is UI metadata — the policy itself is structural (see module
/// docs); [`VaultFile::load_from`] re-validates every invariant on read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultFile {
    pub version: u32,
    /// How many methods must be presented together to unlock (mirrors the
    /// size of every envelope's method set).
    pub required: u8,
    pub methods: Vec<Method>,
    pub envelopes: Vec<Envelope>,
    pub escrows: Vec<Escrow>,
}

/// The result of [`create`]: the fresh vault file, the live VMK, and the
/// recovery key's display string. The display string is the ONLY plaintext
/// copy of the recovery key that will ever exist — the caller shows it once
/// and zeroizes it (`String: Zeroize`); it is never written anywhere.
pub struct NewVault {
    pub file: VaultFile,
    pub vmk: SecretBuf,
    pub recovery_display: String,
}

/// A factor presented at unlock time. Secrets are borrowed — the caller keeps
/// them in locked memory ([`SecretBuf`]) and drops them right after.
pub enum Factor<'a> {
    /// The passphrase, raw bytes (resolved to the enrolled passphrase method).
    Passphrase(&'a [u8]),
    /// The recovery key, raw 32 bytes (from [`parse_recovery`]).
    Recovery(&'a [u8]),
    /// Any method by id with its raw 32-byte secret (passkey PRF output).
    MethodSecret { id: &'a str, secret: &'a [u8] },
}

// ---------------------------------------------------------------------------
// Crypto primitives (thin, on vetted crates — no custom constructions)
// ---------------------------------------------------------------------------

/// Argon2id: passphrase + salt + params → 32-byte wrapping key in locked
/// memory. The block matrix is allocated explicitly and zeroized after the
/// derivation (the crate's plain `hash_password_into` would leave it to the
/// allocator); the crate's `zeroize` feature wipes its internal state hashes.
fn derive_passphrase_key(pass: &[u8], salt: &[u8], kdf: &KdfParams) -> Result<SecretBuf> {
    let params = Params::new(kdf.m_cost_kib, kdf.t_cost, kdf.p_cost, Some(KEY_LEN))
        .map_err(|e| VaultError::Kdf(format!("bad argon2 params: {e}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone());
    let mut out = SecretBuf::zeroed(KEY_LEN)?;
    let mut blocks = vec![Block::default(); params.block_count()];
    let r = argon.hash_password_into_with_memory(pass, salt, out.as_mut_slice(), &mut blocks);
    for b in blocks.iter_mut() {
        b.zeroize();
    }
    r.map_err(|e| VaultError::Kdf(format!("argon2 failed: {e}")))?;
    Ok(out)
}

/// Combine method wrapping keys (in the envelope's sorted-id order) into the
/// envelope key: BLAKE2s-256 over a domain prefix and the concatenated
/// 32-byte keys. All inputs are uniform random keys of fixed length, so the
/// hash is a proper KDF here; single keys pass through the same PRF for
/// domain separation and one code path.
fn combine_keys(parts: &[&SecretBuf]) -> Result<SecretBuf> {
    let mut h = Blake2s256::new();
    h.update(COMBINE_DOMAIN);
    for p in parts {
        h.update(p.as_slice());
    }
    let mut digest: [u8; KEY_LEN] = h.finalize().into();
    let out = SecretBuf::copy_of(&digest);
    digest.zeroize();
    out
}

/// AEAD-wrap `plaintext` (in locked memory) under `key`: returns
/// (nonce, ciphertext ‖ tag). The plaintext is staged into a locked working
/// buffer and encrypted in place, so no cipher-crate allocation ever holds it.
fn aead_wrap(key: &SecretBuf, aad: &[u8], plaintext: &SecretBuf) -> Result<([u8; NONCE_LEN], Vec<u8>)> {
    let nonce: [u8; NONCE_LEN] = rand_array()?;
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_slice()));
    let mut work = SecretBuf::copy_of(plaintext.as_slice())?;
    let tag = cipher
        .encrypt_in_place_detached(XNonce::from_slice(&nonce), aad, work.as_mut_slice())
        .map_err(|_| VaultError::Crypto)?;
    let mut blob = Vec::with_capacity(work.len() + TAG_LEN);
    blob.extend_from_slice(work.as_slice()); // ciphertext — public
    blob.extend_from_slice(&tag);
    Ok((nonce, blob))
}

/// AEAD-unwrap `ciphertext ‖ tag` under `key` into locked memory. Any
/// mismatch (wrong key, tampered blob, wrong AAD) is the uniform
/// [`VaultError::Crypto`].
fn aead_unwrap(key: &SecretBuf, aad: &[u8], nonce: &[u8], blob: &[u8]) -> Result<SecretBuf> {
    if nonce.len() != NONCE_LEN || blob.len() < TAG_LEN + 1 {
        return Err(VaultError::Crypto);
    }
    let (ct, tag) = blob.split_at(blob.len() - TAG_LEN);
    let cipher = XChaCha20Poly1305::new(Key::from_slice(key.as_slice()));
    // Stage the (public) ciphertext into a locked buffer: decrypt-in-place
    // lands the plaintext directly in locked pages.
    let mut work = SecretBuf::copy_of(ct)?;
    cipher
        .decrypt_in_place_detached(
            XNonce::from_slice(nonce),
            aad,
            work.as_mut_slice(),
            Tag::from_slice(tag),
        )
        .map_err(|_| VaultError::Crypto)?;
    Ok(work)
}

fn envelope_aad(method_ids: &[String]) -> Vec<u8> {
    let mut aad = AAD_ENVELOPE.as_bytes().to_vec();
    aad.extend_from_slice(method_ids.join("+").as_bytes());
    aad
}

fn escrow_aad(method_id: &str) -> Vec<u8> {
    let mut aad = AAD_ESCROW.as_bytes().to_vec();
    aad.extend_from_slice(method_id.as_bytes());
    aad
}

// ---------------------------------------------------------------------------
// Recovery-key display format
// ---------------------------------------------------------------------------

/// Crockford base32: no I, L, O, U — nothing to misread off a printout.
const B32_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Encode the 32-byte recovery key for one-time display: key ‖ 3-byte
/// BLAKE2s checksum → 35 bytes → 56 Crockford-base32 chars → 14 groups of 4,
/// dash-separated. `parse_recovery` reverses it, forgiving case, separators
/// and the classic confusables (O→0, I/L→1).
pub fn encode_recovery(key: &[u8]) -> String {
    debug_assert_eq!(key.len(), KEY_LEN);
    let mut data = Vec::with_capacity(KEY_LEN + RECOVERY_CHECK_LEN);
    data.extend_from_slice(key);
    let check: [u8; KEY_LEN] = Blake2s256::digest(key).into();
    data.extend_from_slice(&check[..RECOVERY_CHECK_LEN]);

    // 35 bytes = 280 bits = exactly 56 five-bit symbols.
    let mut out = String::with_capacity(56 + 13);
    let (mut acc, mut bits) = (0u32, 0u32);
    let mut emitted = 0;
    for &b in &data {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32_ALPHABET[((acc >> bits) & 31) as usize] as char);
            emitted += 1;
            if emitted % 4 == 0 && emitted < 56 {
                out.push('-');
            }
        }
    }
    data.zeroize();
    out
}

/// Parse a typed/pasted recovery key back into locked memory. Checksum
/// mismatch is a [`VaultError::Format`] (a local typo signal, distinct from
/// the uniform unlock failure — the checksum is derived from the key alone,
/// so this reveals nothing an attacker holding the string would not know).
pub fn parse_recovery(input: &str) -> Result<SecretBuf> {
    let mut symbols = Vec::with_capacity(56);
    for c in input.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' | '\t' | '\n' | '\r' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            up => up,
        };
        match B32_ALPHABET.iter().position(|&a| a as char == c) {
            Some(v) => symbols.push(v as u8),
            None => return Err(VaultError::Format(format!("invalid character '{c}'"))),
        }
    }
    if symbols.len() != 56 {
        return Err(VaultError::Format(format!(
            "expected 56 characters, got {}",
            symbols.len()
        )));
    }
    let mut data = Vec::with_capacity(KEY_LEN + RECOVERY_CHECK_LEN);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &s in &symbols {
        acc = (acc << 5) | s as u32;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            data.push(((acc >> bits) & 0xff) as u8);
        }
    }
    symbols.zeroize();
    let key = SecretBuf::copy_of(&data[..KEY_LEN])?;
    let check: [u8; KEY_LEN] = Blake2s256::digest(key.as_slice()).into();
    let ok = data[KEY_LEN..] == check[..RECOVERY_CHECK_LEN];
    data.zeroize();
    if !ok {
        return Err(VaultError::Format("checksum mismatch — check for typos".into()));
    }
    Ok(key)
}

// ---------------------------------------------------------------------------
// Envelope-set construction (the policy lives here)
// ---------------------------------------------------------------------------

/// All k-combinations of 0..n (n is tiny — the enrolled-method count).
fn k_combinations(n: usize, k: usize) -> Vec<Vec<usize>> {
    fn rec(start: usize, n: usize, k: usize, cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
        if k == 0 {
            out.push(cur.clone());
            return;
        }
        for i in start..=n - k {
            cur.push(i);
            rec(i + 1, n, k - 1, cur, out);
            cur.pop();
        }
    }
    let mut out = Vec::new();
    if k >= 1 && k <= n {
        rec(0, n, k, &mut Vec::new(), &mut out);
    }
    out
}

/// Build the full vault file from the VMK, every enrolled method WITH its
/// wrapping key, and the policy. This is the single place envelopes and
/// escrows are minted — every mutation (enroll, remove, policy change,
/// rotation) funnels through here, and the never-brick invariant is enforced
/// before anything is returned.
fn rebuild(
    vmk: &SecretBuf,
    methods_keys: &[(Method, SecretBuf)],
    required: u8,
) -> Result<VaultFile> {
    let n = methods_keys.len();
    if required == 0 || required as usize > n {
        return Err(VaultError::Policy(format!(
            "required={required} with {n} enrolled methods"
        )));
    }
    // Deterministic order everywhere: methods sorted by id; an envelope's
    // member list and its key-combination order follow the same sort.
    let mut mk: Vec<&(Method, SecretBuf)> = methods_keys.iter().collect();
    mk.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    if mk.windows(2).any(|w| w[0].0.id == w[1].0.id) {
        return Err(VaultError::Format("duplicate method id".into()));
    }

    let mut envelopes = Vec::new();
    for combo in k_combinations(n, required as usize) {
        let ids: Vec<String> = combo.iter().map(|&i| mk[i].0.id.clone()).collect();
        let keys: Vec<&SecretBuf> = combo.iter().map(|&i| &mk[i].1).collect();
        let combined = combine_keys(&keys)?;
        let (nonce, blob) = aead_wrap(&combined, &envelope_aad(&ids), vmk)?;
        envelopes.push(Envelope {
            method_ids: ids,
            nonce: hex_encode(&nonce),
            wrapped: hex_encode(&blob),
        });
    }

    let mut escrows = Vec::new();
    for (m, key) in mk.iter() {
        let (nonce, blob) = aead_wrap(vmk, &escrow_aad(&m.id), key)?;
        escrows.push(Escrow {
            method_id: m.id.clone(),
            nonce: hex_encode(&nonce),
            wrapped: hex_encode(&blob),
        });
    }

    let file = VaultFile {
        version: VAULT_VERSION,
        required,
        methods: mk.iter().map(|(m, _)| m.clone()).collect(),
        envelopes,
        escrows,
    };
    assert_never_brick(&file)?;
    Ok(file)
}

/// The never-brick invariant (CD-40): the passphrase and the recovery key are
/// always enrolled, the policy is within range, every envelope matches the
/// policy size and references enrolled methods, every method has exactly one
/// escrow — and at least one envelope needs no hardware at all. Checked on
/// every rebuild and on every load, so a violating file is refused before it
/// can strand the user.
pub fn assert_never_brick(file: &VaultFile) -> Result<()> {
    let find = |id: &str| file.methods.iter().find(|m| m.id == id);
    if !file.methods.iter().any(|m| m.kind == MethodKind::Passphrase) {
        return Err(VaultError::Policy("no passphrase method enrolled".into()));
    }
    if !file.methods.iter().any(|m| m.kind == MethodKind::RecoveryKey) {
        return Err(VaultError::Policy("no recovery-key method enrolled".into()));
    }
    if file.required == 0 || file.required as usize > file.methods.len() {
        return Err(VaultError::Policy(format!(
            "required={} out of range for {} methods",
            file.required,
            file.methods.len()
        )));
    }
    if file.envelopes.is_empty() {
        return Err(VaultError::Policy("no envelopes".into()));
    }
    let mut soft_reachable = false;
    for e in &file.envelopes {
        if e.method_ids.len() != file.required as usize {
            return Err(VaultError::Policy(
                "envelope size does not match the unlock policy".into(),
            ));
        }
        let mut all_soft = true;
        for id in &e.method_ids {
            match find(id) {
                Some(m) => all_soft &= !m.kind.hardware(),
                None => {
                    return Err(VaultError::Policy(format!(
                        "envelope references unknown method '{id}'"
                    )));
                }
            }
        }
        soft_reachable |= all_soft;
    }
    if !soft_reachable {
        return Err(VaultError::Policy(
            "no non-hardware unlock path — losing the hardware would brick the vault".into(),
        ));
    }
    for m in &file.methods {
        let n = file.escrows.iter().filter(|e| e.method_id == m.id).count();
        if n != 1 {
            return Err(VaultError::Policy(format!(
                "method '{}' has {n} escrows (expected 1)",
                m.id
            )));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Setup + unlock
// ---------------------------------------------------------------------------

/// Set up a fresh vault: generate the VMK, enroll the passphrase (Argon2id,
/// `kdf`) and mint the recovery key. Two independent envelopes exist from day
/// one; the policy starts at 1-required. The returned recovery display string
/// is shown once and zeroized by the caller — it is never stored.
pub fn create(passphrase: &[u8], kdf: &KdfParams, now_ms: u64) -> Result<NewVault> {
    if passphrase.len() < MIN_PASSPHRASE_LEN {
        return Err(VaultError::Policy(format!(
            "passphrase must be at least {MIN_PASSPHRASE_LEN} bytes"
        )));
    }
    let vmk = SecretBuf::random(KEY_LEN)?;
    let salt: [u8; SALT_LEN] = rand_array()?;
    let pp_key = derive_passphrase_key(passphrase, &salt, kdf)?;
    let rk = SecretBuf::random(KEY_LEN)?;
    let recovery_display = encode_recovery(rk.as_slice());

    let methods_keys = vec![
        (
            Method {
                id: "passphrase".into(),
                kind: MethodKind::Passphrase,
                label: "Passphrase".into(),
                created_ms: now_ms,
                kdf: Some(*kdf),
                salt: Some(hex_encode(&salt)),
            },
            pp_key,
        ),
        (
            Method {
                id: "recovery".into(),
                kind: MethodKind::RecoveryKey,
                label: "Recovery key".into(),
                created_ms: now_ms,
                kdf: None,
                salt: None,
            },
            rk,
        ),
    ];
    let file = rebuild(&vmk, &methods_keys, 1)?;
    Ok(NewVault { file, vmk, recovery_display })
}

/// Resolve presented factors to (method id, wrapping key) pairs.
fn resolve_factors(file: &VaultFile, factors: &[Factor<'_>]) -> Result<Vec<(String, SecretBuf)>> {
    let mut out: Vec<(String, SecretBuf)> = Vec::new();
    for f in factors {
        let (id, key) = match f {
            Factor::Passphrase(pass) => {
                let m = file
                    .methods
                    .iter()
                    .find(|m| m.kind == MethodKind::Passphrase)
                    .ok_or_else(|| VaultError::Format("no passphrase method".into()))?;
                let salt = hex_decode(
                    m.salt
                        .as_deref()
                        .ok_or_else(|| VaultError::Format("passphrase method lacks salt".into()))?,
                )?;
                let kdf = m
                    .kdf
                    .ok_or_else(|| VaultError::Format("passphrase method lacks kdf".into()))?;
                (m.id.clone(), derive_passphrase_key(pass, &salt, &kdf)?)
            }
            Factor::Recovery(rk) => {
                let m = file
                    .methods
                    .iter()
                    .find(|m| m.kind == MethodKind::RecoveryKey)
                    .ok_or_else(|| VaultError::Format("no recovery-key method".into()))?;
                if rk.len() != KEY_LEN {
                    return Err(VaultError::Format("recovery key must be 32 bytes".into()));
                }
                (m.id.clone(), SecretBuf::copy_of(rk)?)
            }
            Factor::MethodSecret { id, secret } => {
                let m = file
                    .methods
                    .iter()
                    .find(|m| m.id == *id)
                    .ok_or_else(|| VaultError::Format(format!("unknown method '{id}'")))?;
                if secret.len() != KEY_LEN {
                    return Err(VaultError::Format("method secret must be 32 bytes".into()));
                }
                (m.id.clone(), SecretBuf::copy_of(secret)?)
            }
        };
        if !out.iter().any(|(i, _)| *i == id) {
            out.push((id, key));
        }
    }
    Ok(out)
}

/// Unlock: try every envelope whose full method set was presented. The policy
/// needs no checking here — at `required = 2` only pair envelopes exist, so a
/// single factor finds no candidate and CANNOT open anything, whatever the
/// mutable `required` field claims. Failure is uniform ([`VaultError::Crypto`]).
pub fn unlock(file: &VaultFile, factors: &[Factor<'_>]) -> Result<SecretBuf> {
    let presented = resolve_factors(file, factors)?;
    for e in &file.envelopes {
        let keys: Option<Vec<&SecretBuf>> = e
            .method_ids
            .iter()
            .map(|id| presented.iter().find(|(i, _)| i == id).map(|(_, k)| k))
            .collect();
        let Some(keys) = keys else { continue };
        let combined = combine_keys(&keys)?;
        let nonce = hex_decode(&e.nonce)?;
        let blob = hex_decode(&e.wrapped)?;
        if let Ok(vmk) = aead_unwrap(&combined, &envelope_aad(&e.method_ids), &nonce, &blob) {
            return Ok(vmk);
        }
    }
    Err(VaultError::Crypto)
}

// ---------------------------------------------------------------------------
// Mutations from an unlocked session (all funnel through rebuild)
// ---------------------------------------------------------------------------

/// Recover every method's wrapping key from its escrow (VMK in memory). The
/// keys live in locked buffers and drop (zeroized) when the mutation is done.
fn unwrap_all_escrows(file: &VaultFile, vmk: &SecretBuf) -> Result<Vec<(Method, SecretBuf)>> {
    let mut out = Vec::with_capacity(file.methods.len());
    for m in &file.methods {
        let e = file
            .escrows
            .iter()
            .find(|e| e.method_id == m.id)
            .ok_or_else(|| VaultError::Format(format!("method '{}' lacks an escrow", m.id)))?;
        let nonce = hex_decode(&e.nonce)?;
        let blob = hex_decode(&e.wrapped)?;
        let key = aead_unwrap(vmk, &escrow_aad(&m.id), &nonce, &blob)?;
        out.push((m.clone(), key));
    }
    Ok(out)
}

/// Enroll a passkey (or any opaque 32-byte method secret) from an unlocked
/// session. Re-wraps the VMK (new envelope set); the vault data is untouched.
pub fn enroll_passkey(
    file: &VaultFile,
    vmk: &SecretBuf,
    label: &str,
    secret: &[u8],
    now_ms: u64,
) -> Result<VaultFile> {
    if secret.len() != KEY_LEN {
        return Err(VaultError::Format("passkey secret must be 32 bytes".into()));
    }
    let id: [u8; 4] = rand_array()?;
    let mut mk = unwrap_all_escrows(file, vmk)?;
    mk.push((
        Method {
            id: format!("passkey-{}", hex_encode(&id)),
            kind: MethodKind::Passkey,
            label: label.to_string(),
            created_ms: now_ms,
            kdf: None,
            salt: None,
        },
        SecretBuf::copy_of(secret)?,
    ));
    rebuild(vmk, &mk, file.required)
}

/// Remove an enrolled method. Stage 1: only hardware methods (passkeys) are
/// removable — the passphrase and the recovery key are the never-brick floor.
pub fn remove_method(file: &VaultFile, vmk: &SecretBuf, id: &str) -> Result<VaultFile> {
    let m = file
        .methods
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| VaultError::Format(format!("unknown method '{id}'")))?;
    if !m.kind.hardware() {
        return Err(VaultError::Policy(
            "the passphrase and the recovery key cannot be removed".into(),
        ));
    }
    let remaining = file.methods.len() - 1;
    if (file.required as usize) > remaining {
        return Err(VaultError::Policy(format!(
            "removing '{id}' would leave {remaining} methods below the {}-required policy",
            file.required
        )));
    }
    let mk: Vec<(Method, SecretBuf)> = unwrap_all_escrows(file, vmk)?
        .into_iter()
        .filter(|(m, _)| m.id != id)
        .collect();
    rebuild(vmk, &mk, file.required)
}

/// Change the unlock policy (how many methods must be presented together).
/// Structural: mints a completely new envelope set of the new combination
/// size. Stage 1's UI offers 1 and 2; the core accepts any 1..=n.
pub fn set_required(file: &VaultFile, vmk: &SecretBuf, required: u8) -> Result<VaultFile> {
    let mk = unwrap_all_escrows(file, vmk)?;
    rebuild(vmk, &mk, required)
}

/// Change the passphrase: fresh salt, fresh Argon2id derivation with `kdf`
/// (also the path for re-tuning the cost parameters), full re-wrap. The old
/// passphrase stops working atomically with the new file.
pub fn change_passphrase(
    file: &VaultFile,
    vmk: &SecretBuf,
    new_passphrase: &[u8],
    kdf: &KdfParams,
    now_ms: u64,
) -> Result<VaultFile> {
    if new_passphrase.len() < MIN_PASSPHRASE_LEN {
        return Err(VaultError::Policy(format!(
            "passphrase must be at least {MIN_PASSPHRASE_LEN} bytes"
        )));
    }
    let salt: [u8; SALT_LEN] = rand_array()?;
    let new_key = derive_passphrase_key(new_passphrase, &salt, kdf)?;
    let mut mk = unwrap_all_escrows(file, vmk)?;
    let slot = mk
        .iter_mut()
        .find(|(m, _)| m.kind == MethodKind::Passphrase)
        .ok_or_else(|| VaultError::Format("no passphrase method".into()))?;
    slot.0.kdf = Some(*kdf);
    slot.0.salt = Some(hex_encode(&salt));
    slot.0.created_ms = now_ms;
    slot.1 = new_key;
    rebuild(vmk, &mk, file.required)
}

/// Mint a fresh recovery key (invalidates the old one atomically) and return
/// its one-time display string alongside the re-wrapped file.
pub fn regenerate_recovery(
    file: &VaultFile,
    vmk: &SecretBuf,
    now_ms: u64,
) -> Result<(VaultFile, String)> {
    let rk = SecretBuf::random(KEY_LEN)?;
    let display = encode_recovery(rk.as_slice());
    let mut mk = unwrap_all_escrows(file, vmk)?;
    let slot = mk
        .iter_mut()
        .find(|(m, _)| m.kind == MethodKind::RecoveryKey)
        .ok_or_else(|| VaultError::Format("no recovery-key method".into()))?;
    slot.0.created_ms = now_ms;
    slot.1 = rk;
    let new_file = rebuild(vmk, &mk, file.required)?;
    Ok((new_file, display))
}

// ---------------------------------------------------------------------------
// Sealed app state (encrypted at rest under the VMK)
// ---------------------------------------------------------------------------

/// Seal a state blob under the VMK: magic ‖ nonce ‖ ciphertext ‖ tag. The
/// magic doubles as AAD, so a sealed-state blob can never be confused with an
/// envelope or escrow.
pub fn seal_state(vmk: &SecretBuf, plaintext: &[u8]) -> Result<Vec<u8>> {
    let work = SecretBuf::copy_of(plaintext)?;
    let (nonce, blob) = aead_wrap(vmk, SEAL_MAGIC, &work)?;
    let mut out = Vec::with_capacity(SEAL_MAGIC.len() + NONCE_LEN + blob.len());
    out.extend_from_slice(SEAL_MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&blob);
    Ok(out)
}

/// Open a sealed state blob into locked memory. Fails closed on any
/// truncation, wrong magic, wrong key or tampering.
pub fn open_state(vmk: &SecretBuf, data: &[u8]) -> Result<SecretBuf> {
    if data.len() < SEAL_MAGIC.len() + NONCE_LEN + TAG_LEN + 1 {
        return Err(VaultError::Crypto);
    }
    let (magic, rest) = data.split_at(SEAL_MAGIC.len());
    if magic != SEAL_MAGIC {
        return Err(VaultError::Crypto);
    }
    let (nonce, blob) = rest.split_at(NONCE_LEN);
    aead_unwrap(vmk, SEAL_MAGIC, nonce, blob)
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// `vault.json` — envelope metadata (public by design; see [`VaultFile`]).
pub fn vault_file_path() -> PathBuf {
    crate::store::data_dir().join("vault.json")
}

/// `vault.seal` — the sealed sensitive app state (Stage 1b wires tenants in).
pub fn sealed_state_path() -> PathBuf {
    crate::store::data_dir().join("vault.seal")
}

impl VaultFile {
    /// Load and validate a vault file. `Ok(None)` when none exists (no vault
    /// set up). A file that parses but violates the format version or the
    /// never-brick invariant is refused — fail closed, never boot into a
    /// state that could strand the user deeper in.
    pub fn load_from(path: &Path) -> Result<Option<VaultFile>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(VaultError::Io(format!("read {}: {e}", path.display()))),
        };
        let file: VaultFile =
            serde_json::from_str(&raw).map_err(|e| VaultError::Format(format!("vault.json: {e}")))?;
        if file.version != VAULT_VERSION {
            return Err(VaultError::Format(format!(
                "unsupported vault version {} (this build reads {VAULT_VERSION})",
                file.version
            )));
        }
        assert_never_brick(&file)?;
        Ok(Some(file))
    }

    /// Atomic save: write a sibling temp file, then rename over the target
    /// (`std::fs::rename` replaces on Windows). A crash mid-write leaves the
    /// previous consistent file in place — key management must never be
    /// half-written.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        assert_never_brick(self)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| VaultError::Format(format!("serialize: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| VaultError::Io(format!("write tmp: {e}")))?;
        std::fs::rename(&tmp, path).map_err(|e| VaultError::Io(format!("rename: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SecretInput — host-captured secret entry (Stage 1b)
// ---------------------------------------------------------------------------

/// A typed-in secret being assembled in locked memory. This is the iron-law
/// mechanism: while the vault captures input, the HOST consumes the window's
/// key events and appends them here — the lock/settings page never receives a
/// keystroke, only a masked character COUNT to render. Fixed capacity (one
/// page); UTF-8 by construction.
pub struct SecretInput {
    buf: SecretBuf,
    len: usize,
}

impl SecretInput {
    const CAP: usize = 1024;

    pub fn new() -> Result<Self> {
        Ok(Self { buf: SecretBuf::zeroed(Self::CAP)?, len: 0 })
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.buf.as_slice()[..self.len]
    }

    fn as_str(&self) -> &str {
        // Valid UTF-8 by construction: only whole `&str`s are ever appended.
        std::str::from_utf8(self.as_slice()).unwrap_or("")
    }

    /// Number of CHARACTERS typed (what the page renders as dots).
    pub fn chars(&self) -> usize {
        self.as_str().chars().count()
    }

    /// Append typed text; silently ignores input past capacity (a 1 KiB
    /// passphrase is a keyboard on the desk, not a user).
    pub fn push_str(&mut self, s: &str) {
        for ch in s.chars() {
            // Control characters never enter a secret (Enter/Esc/Backspace are
            // handled as commands; stray control input is noise).
            if ch.is_control() {
                continue;
            }
            let mut enc = [0u8; 4];
            let bytes = ch.encode_utf8(&mut enc).as_bytes();
            if self.len + bytes.len() > Self::CAP {
                return;
            }
            self.buf.as_mut_slice()[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
            enc.zeroize();
        }
    }

    /// Remove the last character (UTF-8-aware: walks back over continuation
    /// bytes, then wipes the freed tail).
    pub fn backspace(&mut self) {
        if self.len == 0 {
            return;
        }
        let s = self.buf.as_slice();
        let mut i = self.len - 1;
        while i > 0 && (s[i] & 0b1100_0000) == 0b1000_0000 {
            i -= 1;
        }
        let old = self.len;
        self.len = i;
        self.buf.as_mut_slice()[i..old].zeroize();
    }

    pub fn clear(&mut self) {
        self.buf.wipe();
        self.len = 0;
    }
}

// ---------------------------------------------------------------------------
// Runtime (Stage 1b): lock state, capture state machine, workers, sealed state
// ---------------------------------------------------------------------------
//
// One process-wide runtime behind one Mutex, following the store.rs precedent.
// Called from the main thread (key routing, boot transitions in about_to_wait)
// and the CEF UI thread (the vault IPC commands); the worker threads lock only
// to COMMIT results — the Argon2 derivation itself runs without any lock held
// (CD-38 threading law: nothing here is ever awaited on the router's dispatch
// stack, and no vault lock is held across a CEF call).

/// What the host is currently capturing keystrokes for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureKind {
    /// The passphrase, to unlock (step 1 when the policy requires 2).
    UnlockPass,
    /// The recovery key, to unlock (alone, or step 2 of a 2-required unlock).
    UnlockRecovery,
    /// The new passphrase during vault setup…
    SetupPass,
    /// …and its confirmation re-type (internal step, never begun via IPC).
    SetupConfirm,
}

impl CaptureKind {
    fn as_str(self) -> &'static str {
        match self {
            CaptureKind::UnlockPass => "unlock_pass",
            CaptureKind::UnlockRecovery => "unlock_recovery",
            CaptureKind::SetupPass => "setup_pass",
            CaptureKind::SetupConfirm => "setup_confirm",
        }
    }
}

/// A finished background operation, taken by the shell (`about_to_wait`) to
/// drive the UI transition. The VMK itself never rides an outcome — the worker
/// commits it straight into the runtime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The vault unlocked — boot the workspace.
    Unlocked,
    /// An unlock attempt failed (the uniform error is already in the state).
    UnlockFailed,
    /// Setup finished — the vault exists, the session is unlocked, and the
    /// one-time recovery display is in the state until acked.
    SetupDone,
    /// Setup failed (error in the state).
    SetupFailed,
}

struct Runtime {
    /// Directory holding `vault.json` / `vault.seal` (the app-data dir; tests
    /// point it at a temp dir).
    dir: PathBuf,
    file: Option<VaultFile>,
    /// A vault file exists but could not be validated (tamper/corruption).
    /// Fail-closed: the gate stays locked and unlock cannot succeed — booting
    /// as "no vault" on a broken file would let corruption bypass the gate.
    broken: Option<String>,
    vmk: Option<SecretBuf>,
    /// Dev bypass engaged (debug builds only): the gate is skipped, the
    /// sealed state stays sealed.
    bypassed: bool,
    capture: Option<CaptureKind>,
    input: Option<SecretInput>,
    /// The passphrase held between step 1 and step 2 of a 2-required unlock.
    pending_pass: Option<SecretInput>,
    busy: bool,
    error: Option<String>,
    /// The one-time recovery display after setup, cleared on ack.
    recovery_display: Option<String>,
    outcome: Option<Outcome>,
    relaunch: bool,
    /// The decrypted sealed app state (`vault.seal`), present only while
    /// unlocked. JSON object; tenants: identity_seed, identity_seed_born.
    sealed: Option<serde_json::Value>,
    /// KDF cost for setup (product default; tests override).
    kdf: KdfParams,
}

fn rt() -> &'static Mutex<Runtime> {
    use std::sync::OnceLock;
    static RT: OnceLock<Mutex<Runtime>> = OnceLock::new();
    RT.get_or_init(|| {
        Mutex::new(Runtime {
            dir: crate::store::data_dir(),
            file: None,
            broken: None,
            vmk: None,
            bypassed: false,
            capture: None,
            input: None,
            pending_pass: None,
            busy: false,
            error: None,
            recovery_display: None,
            outcome: None,
            relaunch: false,
            sealed: None,
            kdf: KdfParams::PRODUCT,
        })
    })
}

/// Load the vault state at boot (after `settings::init`, before any view).
/// With a valid vault present the app starts LOCKED. The dev bypass
/// (`CYBERDESK_VAULT_BYPASS=1`) exists ONLY in debug builds — the check is
/// `cfg(debug_assertions)`-gated, so a release artifact contains no bypass
/// code path at all; it skips the GATE, never the cryptography (the sealed
/// state stays sealed — the VMK cannot be conjured).
pub fn init() {
    let mut r = rt().lock().unwrap();
    let path = r.dir.join("vault.json");
    match VaultFile::load_from(&path) {
        Ok(file) => r.file = file,
        Err(e) => {
            tracing::error!("vault.json failed to load — staying locked: {e}");
            r.broken = Some(e.to_string());
        }
    }
    #[cfg(debug_assertions)]
    if (r.file.is_some() || r.broken.is_some())
        && std::env::var("CYBERDESK_VAULT_BYPASS").as_deref() == Ok("1")
    {
        tracing::warn!(
            "VAULT DEV BYPASS ACTIVE (debug build): gate skipped, sealed state stays sealed"
        );
        r.bypassed = true;
    }
    if r.file.is_some() && !r.bypassed {
        tracing::info!("vault present — starting locked");
    }
}

pub fn has_vault() -> bool {
    let r = rt().lock().unwrap();
    r.file.is_some() || r.broken.is_some()
}

/// Is the start-authorization gate closed? (True boots the lock screen.)
pub fn is_locked() -> bool {
    let r = rt().lock().unwrap();
    (r.file.is_some() || r.broken.is_some()) && r.vmk.is_none() && !r.bypassed
}

/// Is a vault present AND open (VMK in memory)?
pub fn is_unlocked() -> bool {
    let r = rt().lock().unwrap();
    r.file.is_some() && r.vmk.is_some()
}

/// Begin capturing a secret on the host. Valid purposes from the IPC:
/// `unlock_pass` / `unlock_recovery` (locked only), `setup_pass` (no vault
/// only, or explicitly while unlocked later for re-tuning — Stage 1c).
pub fn begin_capture(purpose: &str) -> std::result::Result<(), String> {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return Err("busy".into());
    }
    let kind = match purpose {
        "unlock_pass" => CaptureKind::UnlockPass,
        "unlock_recovery" => CaptureKind::UnlockRecovery,
        "setup_pass" => CaptureKind::SetupPass,
        other => return Err(format!("unknown capture purpose: {other}")),
    };
    match kind {
        CaptureKind::UnlockPass | CaptureKind::UnlockRecovery => {
            if r.vmk.is_some() || (r.file.is_none() && r.broken.is_none()) {
                return Err("not locked".into());
            }
        }
        CaptureKind::SetupPass => {
            if r.file.is_some() || r.broken.is_some() {
                return Err("a vault already exists".into());
            }
        }
        CaptureKind::SetupConfirm => unreachable!(),
    }
    let input = SecretInput::new().map_err(|e| e.to_string())?;
    r.capture = Some(kind);
    r.input = Some(input);
    r.pending_pass = None;
    r.error = None;
    Ok(())
}

/// Cancel the current capture. While locked this resets to a fresh
/// passphrase prompt; elsewhere it ends the flow.
pub fn cancel_capture() {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return;
    }
    r.input = None;
    r.pending_pass = None;
    r.error = None;
    let locked = (r.file.is_some() || r.broken.is_some()) && r.vmk.is_none() && !r.bypassed;
    if locked {
        r.capture = Some(CaptureKind::UnlockPass);
        r.input = SecretInput::new().ok();
    } else {
        r.capture = None;
    }
}

/// Is the host currently swallowing keystrokes into a secret buffer?
pub fn capture_active() -> bool {
    let r = rt().lock().unwrap();
    r.capture.is_some() && !r.busy
}

/// Route typed text into the capture buffer (also the paste path).
pub fn key_text(text: &str) {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return;
    }
    if let Some(input) = r.input.as_mut() {
        input.push_str(text);
    }
}

pub fn key_backspace() {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return;
    }
    if let Some(input) = r.input.as_mut() {
        input.backspace();
    }
}

/// Enter: advance the capture state machine. Cheap validations happen here;
/// anything with an Argon2 in it goes to a worker thread — the render loop
/// must never stall on a derivation.
pub fn key_submit() {
    let mut r = rt().lock().unwrap();
    if r.busy || r.capture.is_none() {
        return;
    }
    match r.capture.unwrap() {
        CaptureKind::UnlockPass => {
            let required = r.file.as_ref().map(|f| f.required).unwrap_or(1);
            if r.input.as_ref().map(|i| i.len).unwrap_or(0) == 0 {
                return; // empty Enter: nothing to do
            }
            if required >= 2 {
                // Step 1 of 2 banked; the recovery key is step 2 (the passkey
                // joins the step list in the final sub-stage).
                r.pending_pass = r.input.take();
                r.input = SecretInput::new().ok();
                r.capture = Some(CaptureKind::UnlockRecovery);
                r.error = None;
            } else {
                let pass = r.input.take();
                spawn_unlock(&mut r, pass, None);
            }
        }
        CaptureKind::UnlockRecovery => {
            let Some(input) = r.input.take() else { return };
            match parse_recovery(input.as_str()) {
                Ok(rk) => {
                    let pass = r.pending_pass.take();
                    spawn_unlock(&mut r, pass, Some(rk));
                }
                Err(e) => {
                    // A local format/checksum error — no crypto ran; let the
                    // user fix the typo without re-entering everything.
                    r.error = Some(e.to_string());
                    r.input = SecretInput::new().ok();
                }
            }
        }
        CaptureKind::SetupPass => {
            let len = r.input.as_ref().map(|i| i.len).unwrap_or(0);
            if len < MIN_PASSPHRASE_LEN {
                r.error = Some(format!(
                    "passphrase must be at least {MIN_PASSPHRASE_LEN} characters"
                ));
                return;
            }
            r.pending_pass = r.input.take();
            r.input = SecretInput::new().ok();
            r.capture = Some(CaptureKind::SetupConfirm);
            r.error = None;
        }
        CaptureKind::SetupConfirm => {
            let confirm = r.input.take();
            let first = r.pending_pass.take();
            let (Some(first), Some(confirm)) = (first, confirm) else { return };
            if first.as_slice() != confirm.as_slice() {
                r.error = Some("the two entries do not match — start again".into());
                r.capture = Some(CaptureKind::SetupPass);
                r.input = SecretInput::new().ok();
                return;
            }
            drop(confirm);
            spawn_setup(&mut r, first);
        }
    }
}

/// Unlock on a worker thread: derive → try envelopes → open the sealed state
/// → commit. The runtime lock is NOT held during the derivation.
fn spawn_unlock(r: &mut Runtime, pass: Option<SecretInput>, rk: Option<SecretBuf>) {
    let Some(file) = r.file.clone() else {
        // Broken vault file: unlock is impossible by design (fail-closed).
        r.error = Some(r.broken.clone().unwrap_or_else(|| "vault unavailable".into()));
        r.outcome = Some(Outcome::UnlockFailed);
        r.input = SecretInput::new().ok();
        return;
    };
    r.busy = true;
    r.error = None;
    let dir = r.dir.clone();
    std::thread::spawn(move || {
        let mut factors: Vec<Factor<'_>> = Vec::new();
        if let Some(p) = pass.as_ref() {
            factors.push(Factor::Passphrase(p.as_slice()));
        }
        if let Some(k) = rk.as_ref() {
            factors.push(Factor::Recovery(k.as_slice()));
        }
        let result = unlock(&file, &factors);
        drop(factors);
        let sealed = result.as_ref().ok().map(|vmk| load_sealed(&dir, vmk));
        let mut r = rt().lock().unwrap();
        r.busy = false;
        match result {
            Ok(vmk) => {
                r.vmk = Some(vmk);
                r.sealed = sealed;
                r.capture = None;
                r.input = None;
                r.pending_pass = None;
                r.error = None;
                r.outcome = Some(Outcome::Unlocked);
                tracing::info!("vault unlocked");
            }
            Err(_) => {
                // Uniform by design (VaultError::Crypto carries no oracle).
                r.error = Some("unlock failed".into());
                r.capture = Some(CaptureKind::UnlockPass);
                r.input = SecretInput::new().ok();
                r.outcome = Some(Outcome::UnlockFailed);
            }
        }
    });
}

/// Set up the vault on a worker thread: create → save `vault.json` → migrate
/// the plaintext identity seed into the sealed state → commit an UNLOCKED
/// session with the one-time recovery display.
fn spawn_setup(r: &mut Runtime, pass: SecretInput) {
    r.busy = true;
    r.error = None;
    let dir = r.dir.clone();
    let kdf = r.kdf;
    std::thread::spawn(move || {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let result = create(pass.as_slice(), &kdf, now_ms)
            .and_then(|nv| nv.file.save_to(&dir.join("vault.json")).map(|()| nv));
        drop(pass);
        let committed = match result {
            Ok(nv) => nv,
            Err(e) => {
                let mut r = rt().lock().unwrap();
                r.busy = false;
                r.error = Some(e.to_string());
                r.capture = Some(CaptureKind::SetupPass);
                r.input = SecretInput::new().ok();
                r.outcome = Some(Outcome::SetupFailed);
                return;
            }
        };
        // Migrate what is sensitive TODAY into the sealed state: the persisted
        // identity seed (it keys the fingerprint farbling — linkage material),
        // then remove the plaintext rows. Session/layout metadata stays in
        // state.db per the ticket (do not seal what doesn't need sealing).
        // Not under test: the unit suite must never open (or delete rows from)
        // the developer's real state.db — the runtime test drives the sealed
        // tenants through sealed_set/sealed_get instead.
        let mut sealed = serde_json::json!({});
        #[cfg(not(test))]
        {
            let store = crate::store::shared().lock().unwrap();
            if let Some(seed) = store.get("identity_seed") {
                sealed["identity_seed"] = serde_json::Value::String(seed);
                store.delete("identity_seed");
            }
            if let Some(born) = store.get("identity_seed_born") {
                sealed["identity_seed_born"] = serde_json::Value::String(born);
                store.delete("identity_seed_born");
            }
        }
        let NewVault { file, vmk, recovery_display } = committed;
        if let Err(e) = save_sealed(&dir, &vmk, &sealed) {
            tracing::error!("sealed-state write failed at setup: {e}");
        }
        let mut r = rt().lock().unwrap();
        r.busy = false;
        r.file = Some(file);
        r.vmk = Some(vmk);
        r.sealed = Some(sealed);
        r.capture = None;
        r.input = None;
        r.pending_pass = None;
        r.recovery_display = Some(recovery_display);
        r.outcome = Some(Outcome::SetupDone);
        tracing::info!("vault created — session unlocked");
    });
}

/// The user confirmed saving the recovery key: drop the one-time display.
pub fn setup_ack() {
    let mut r = rt().lock().unwrap();
    if let Some(mut s) = r.recovery_display.take() {
        s.zeroize();
    }
}

/// Queue "lock now": the shell relaunches the process cold (D-0059) — every
/// renderer dies with it, and the next boot IS the gate.
pub fn request_lock() {
    rt().lock().unwrap().relaunch = true;
}

pub fn take_relaunch() -> bool {
    let mut r = rt().lock().unwrap();
    std::mem::take(&mut r.relaunch)
}

pub fn take_outcome() -> Option<Outcome> {
    rt().lock().unwrap().outcome.take()
}

/// Best-effort zeroize of everything secret before a deliberate exit/relaunch
/// (statics never drop on process exit, so this is called explicitly; an
/// abnormal termination is covered by the OS zeroing freed pages — the CD-33
/// tier model).
pub fn wipe_for_exit() {
    let mut r = rt().lock().unwrap();
    r.vmk = None;
    r.input = None;
    r.pending_pass = None;
    r.sealed = None;
    if let Some(mut s) = r.recovery_display.take() {
        s.zeroize();
    }
}

/// The vault state snapshot the lock/settings pages render (pushed on change
/// via `browser::set_vault_state`, pulled on load via `get_vault_state`).
/// Carries counts and states — NEVER a secret. (The one deliberate exception
/// is the one-time recovery display after setup: user-facing by definition —
/// it exists to leave the machine on paper — and dropped on ack.)
pub fn state_json() -> String {
    let r = rt().lock().unwrap();
    let vault = if r.vmk.is_some() {
        "unlocked"
    } else if r.bypassed {
        "bypassed"
    } else if r.file.is_some() || r.broken.is_some() {
        "locked"
    } else {
        "none"
    };
    serde_json::json!({
        "vault": vault,
        "capture": r.capture.map(|c| c.as_str()),
        "chars": r.input.as_ref().map(|i| i.chars()).unwrap_or(0),
        "step2": r.pending_pass.is_some(),
        "required": r.file.as_ref().map(|f| f.required).unwrap_or(1),
        "busy": r.busy,
        "error": r.error,
        "recovery": r.recovery_display,
        "broken": r.broken,
    })
    .to_string()
}

// --- Sealed-state tenants ---------------------------------------------------

fn load_sealed(dir: &Path, vmk: &SecretBuf) -> serde_json::Value {
    let path = dir.join("vault.seal");
    match std::fs::read(&path) {
        Ok(data) => match open_state(vmk, &data) {
            Ok(plain) => serde_json::from_slice(plain.as_slice())
                .unwrap_or_else(|_| serde_json::json!({})),
            Err(e) => {
                tracing::error!("sealed state failed to open (tamper/corruption): {e}");
                serde_json::json!({})
            }
        },
        Err(_) => serde_json::json!({}),
    }
}

fn save_sealed(dir: &Path, vmk: &SecretBuf, value: &serde_json::Value) -> Result<()> {
    let plain = serde_json::to_vec(value).map_err(|e| VaultError::Format(e.to_string()))?;
    let blob = seal_state(vmk, &plain)?;
    let path = dir.join("vault.seal");
    let tmp = dir.join("vault.seal.tmp");
    std::fs::write(&tmp, &blob).map_err(|e| VaultError::Io(format!("write tmp: {e}")))?;
    std::fs::rename(&tmp, &path).map_err(|e| VaultError::Io(format!("rename: {e}")))?;
    Ok(())
}

/// Read a sealed string tenant (unlocked sessions only — a locked or
/// bypassed vault yields None, never a plaintext fallback).
pub fn sealed_get(key: &str) -> Option<String> {
    let r = rt().lock().unwrap();
    let sealed = r.sealed.as_ref()?;
    r.vmk.as_ref()?;
    sealed.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Write a sealed string tenant and persist the sealed state (unlocked
/// sessions only; a no-op otherwise — fail-closed, nothing falls back to
/// plaintext).
pub fn sealed_set(key: &str, value: &str) {
    let mut r = rt().lock().unwrap();
    if r.vmk.is_none() {
        return;
    }
    if r.sealed.is_none() {
        r.sealed = Some(serde_json::json!({}));
    }
    if let Some(sealed) = r.sealed.as_mut() {
        sealed[key] = serde_json::Value::String(value.to_string());
    }
    let (dir, sealed) = (r.dir.clone(), r.sealed.clone().unwrap());
    if let Some(vmk) = r.vmk.as_ref()
        && let Err(e) = save_sealed(&dir, vmk, &sealed)
    {
        tracing::error!("sealed-state write failed: {e}");
    }
}

#[cfg(test)]
fn test_reset_runtime(dir: &Path, kdf: KdfParams) {
    let mut r = rt().lock().unwrap();
    *r = Runtime {
        dir: dir.to_path_buf(),
        file: None,
        broken: None,
        vmk: None,
        bypassed: false,
        capture: None,
        input: None,
        pending_pass: None,
        busy: false,
        error: None,
        recovery_display: None,
        outcome: None,
        relaunch: false,
        sealed: None,
        kdf,
    };
    let path = dir.join("vault.json");
    if let Ok(file) = VaultFile::load_from(&path) {
        r.file = file;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Fast Argon2 params for tests only — the code path is identical (params
    /// are stored data), the cost is not what these tests assert.
    const TEST_KDF: KdfParams = KdfParams { m_cost_kib: 64, t_cost: 1, p_cost: 1 };
    const NOW: u64 = 1_753_000_000_000;

    fn fresh() -> NewVault {
        create(b"correct horse battery staple", &TEST_KDF, NOW).expect("create")
    }

    // --- SecretBuf ----------------------------------------------------------

    /// Locked allocation arrives zeroed, holds data, wipes on demand — and
    /// construction succeeding IS the lock guarantee (zeroed() fails closed
    /// when VirtualLock refuses).
    #[test]
    fn secretbuf_allocates_locked_zeroed_and_wipes() {
        let mut b = SecretBuf::zeroed(48).expect("locked alloc");
        assert_eq!(b.len(), 48);
        assert!(b.as_slice().iter().all(|&x| x == 0), "fresh pages are zeroed");
        b.as_mut_slice()[..4].copy_from_slice(&[1, 2, 3, 4]);
        assert_eq!(&b.as_slice()[..4], &[1, 2, 3, 4]);
        b.wipe();
        assert!(b.as_slice().iter().all(|&x| x == 0), "wipe zeroes in place");

        // A burst of buffers (one unlock's worth and then some) must all lock.
        let burst: Vec<SecretBuf> = (0..16).map(|_| SecretBuf::random(KEY_LEN).unwrap()).collect();
        assert!(burst.iter().all(|s| s.len() == KEY_LEN));
    }

    // --- Recovery-key display format ---------------------------------------

    #[test]
    fn recovery_display_round_trips_and_checksum_catches_typos() {
        let rk = SecretBuf::random(KEY_LEN).unwrap();
        let shown = encode_recovery(rk.as_slice());
        // 14 dash-separated groups of 4 Crockford symbols.
        assert_eq!(shown.split('-').count(), 14);
        assert!(shown.split('-').all(|g| g.len() == 4));

        let parsed = parse_recovery(&shown).expect("clean round-trip");
        assert_eq!(parsed.as_slice(), rk.as_slice());

        // Forgiving input: lowercase, spaces instead of dashes, confusables.
        let sloppy = shown.to_lowercase().replace('-', " ").replace('0', "o");
        assert_eq!(parse_recovery(&sloppy).unwrap().as_slice(), rk.as_slice());

        // A single-symbol typo is caught locally by the checksum.
        let mut chars: Vec<char> = shown.chars().collect();
        let pos = chars.iter().position(|&c| c != '-').unwrap();
        chars[pos] = if chars[pos] == '7' { '9' } else { '7' };
        let typo: String = chars.into_iter().collect();
        assert!(matches!(parse_recovery(&typo), Err(VaultError::Format(_))));

        // Wrong length is a format error, not a crypto error.
        assert!(matches!(parse_recovery("ABCD-1234"), Err(VaultError::Format(_))));
    }

    // --- Setup + unlock ------------------------------------------------------

    /// CD-40 acceptance 1+2 (headless half): setup creates a VMK with a
    /// passphrase envelope and a displayed recovery key; the passphrase
    /// unlocks, the recovery key unlocks independently.
    #[test]
    fn create_unlocks_with_passphrase_and_with_recovery_key() {
        let nv = fresh();
        assert_eq!(nv.file.methods.len(), 2, "passphrase + recovery from day one");
        assert_eq!(nv.file.envelopes.len(), 2, "two independent envelopes at 1-required");
        assert_eq!(nv.file.required, 1);

        let vmk = unlock(&nv.file, &[Factor::Passphrase(b"correct horse battery staple")])
            .expect("passphrase unlocks");
        assert_eq!(vmk.as_slice(), nv.vmk.as_slice());

        let rk = parse_recovery(&nv.recovery_display).unwrap();
        let vmk2 = unlock(&nv.file, &[Factor::Recovery(rk.as_slice())])
            .expect("recovery key unlocks independently");
        assert_eq!(vmk2.as_slice(), nv.vmk.as_slice());
    }

    /// Wrong factors and tampered blobs all fail with the uniform crypto
    /// error — no oracle distinguishes them.
    #[test]
    fn wrong_passphrase_and_tampered_envelope_fail_closed() {
        let nv = fresh();
        assert_eq!(
            unlock(&nv.file, &[Factor::Passphrase(b"wrong horse battery staple")]).unwrap_err(),
            VaultError::Crypto
        );

        // Flip one ciphertext byte in every envelope → nothing unlocks.
        let mut tampered = nv.file.clone();
        for e in &mut tampered.envelopes {
            let mut blob = hex_decode(&e.wrapped).unwrap();
            blob[0] ^= 0x01;
            e.wrapped = hex_encode(&blob);
        }
        assert_eq!(
            unlock(&tampered, &[Factor::Passphrase(b"correct horse battery staple")]).unwrap_err(),
            VaultError::Crypto
        );

        // Renaming an envelope's method set breaks the AAD binding even with
        // the right factor and untouched ciphertext.
        let mut renamed = nv.file.clone();
        for e in &mut renamed.envelopes {
            if e.method_ids == vec!["recovery".to_string()] {
                e.method_ids = vec!["passphrase".to_string()];
            }
        }
        renamed.envelopes.retain(|e| e.method_ids == vec!["passphrase".to_string()]);
        // Both remaining envelopes claim "passphrase"; the transplanted one
        // must fail its AAD check, the genuine one only opens with the real
        // passphrase — so the recovery key opens nothing anymore.
        let rk = parse_recovery(&nv.recovery_display).unwrap();
        assert!(unlock(&renamed, &[Factor::Recovery(rk.as_slice())]).is_err());
    }

    /// CD-40 acceptance 4 (headless half): the 2-required policy is enforced
    /// STRUCTURALLY — single factors find no envelope, and hand-editing the
    /// `required` field back to 1 changes nothing because no single-method
    /// envelope exists.
    #[test]
    fn two_required_policy_is_structural_not_a_flag() {
        let nv = fresh();
        let two = set_required(&nv.file, &nv.vmk, 2).expect("policy re-wrap");
        assert_eq!(two.required, 2);
        assert_eq!(two.envelopes.len(), 1, "one pair envelope for {{pp, rk}}");

        let rk = parse_recovery(&nv.recovery_display).unwrap();
        assert!(unlock(&two, &[Factor::Passphrase(b"correct horse battery staple")]).is_err());
        assert!(unlock(&two, &[Factor::Recovery(rk.as_slice())]).is_err());
        let vmk = unlock(
            &two,
            &[
                Factor::Passphrase(b"correct horse battery staple"),
                Factor::Recovery(rk.as_slice()),
            ],
        )
        .expect("both together unlock");
        assert_eq!(vmk.as_slice(), nv.vmk.as_slice());

        // The attacker edit: required 2 → 1. Still only pair envelopes exist.
        let mut edited = two.clone();
        edited.required = 1;
        assert!(
            unlock(&edited, &[Factor::Passphrase(b"correct horse battery staple")]).is_err(),
            "downgrading the flag must not weaken the cryptography"
        );

        // And back down to 1-required through the proper path.
        let one = set_required(&two, &nv.vmk, 1).expect("back to 1-required");
        assert!(unlock(&one, &[Factor::Passphrase(b"correct horse battery staple")]).is_ok());
    }

    /// CD-40 acceptance 3 (core half): a passkey-style method enrolls from an
    /// unlocked session (escrows make the re-wrap possible with only the VMK
    /// in memory), unlocks independently at 1-required, and is removable.
    #[test]
    fn passkey_enrolls_from_unlocked_session_and_unlocks_independently() {
        let nv = fresh();
        let prf = SecretBuf::random(KEY_LEN).unwrap(); // stands in for the PRF output
        let with_pk = enroll_passkey(&nv.file, &nv.vmk, "YubiKey 5", prf.as_slice(), NOW)
            .expect("enroll from unlocked session");
        assert_eq!(with_pk.methods.len(), 3);
        assert_eq!(with_pk.envelopes.len(), 3, "one envelope per method at 1-required");

        let pk_id = with_pk
            .methods
            .iter()
            .find(|m| m.kind == MethodKind::Passkey)
            .unwrap()
            .id
            .clone();
        let vmk = unlock(
            &with_pk,
            &[Factor::MethodSecret { id: &pk_id, secret: prf.as_slice() }],
        )
        .expect("passkey unlocks alone at 1-required");
        assert_eq!(vmk.as_slice(), nv.vmk.as_slice());

        // Old (pre-enrollment) file still works — enrollment re-wrapped, it
        // never re-encrypted anything the old envelopes covered.
        assert!(unlock(&nv.file, &[Factor::Passphrase(b"correct horse battery staple")]).is_ok());

        // Removal: the passkey secret stops working against the new file.
        let removed = remove_method(&with_pk, &nv.vmk, &pk_id).expect("passkey is removable");
        assert_eq!(removed.methods.len(), 2);
        assert!(
            unlock(&removed, &[Factor::MethodSecret { id: &pk_id, secret: prf.as_slice() }])
                .is_err()
        );
    }

    /// At 2-required with a passkey enrolled, every pair gets an envelope —
    /// including the non-hardware {passphrase, recovery} pair the never-brick
    /// rule demands. And a 3-required future policy works today (acceptance:
    /// "the envelope design must allow more later").
    #[test]
    fn multi_factor_combinations_cover_the_non_hardware_pair() {
        let nv = fresh();
        let prf = SecretBuf::random(KEY_LEN).unwrap();
        let with_pk = enroll_passkey(&nv.file, &nv.vmk, "Key", prf.as_slice(), NOW).unwrap();

        let two = set_required(&with_pk, &nv.vmk, 2).unwrap();
        assert_eq!(two.envelopes.len(), 3, "C(3,2) pair envelopes");
        let rk = parse_recovery(&nv.recovery_display).unwrap();
        assert!(
            unlock(
                &two,
                &[
                    Factor::Passphrase(b"correct horse battery staple"),
                    Factor::Recovery(rk.as_slice()),
                ],
            )
            .is_ok(),
            "losing the hardware still leaves the non-hardware pair"
        );

        // 3-required over {passphrase, recovery, passkey} would make EVERY
        // envelope contain the hardware token — losing it would brick the
        // vault. The never-brick invariant refuses the policy outright: the
        // required count may only rise while a non-hardware combination still
        // exists.
        assert!(matches!(set_required(&two, &nv.vmk, 3), Err(VaultError::Policy(_))));
    }

    /// "The envelope design must allow more later": N-required beyond 2 works
    /// today whenever N non-hardware methods exist — proven with a synthetic
    /// third soft method, unlocked only by all three factors together.
    #[test]
    fn n_required_generalizes_beyond_two_with_enough_soft_methods() {
        let vmk = SecretBuf::random(KEY_LEN).unwrap();
        let (k1, k2, k3) = (
            SecretBuf::random(KEY_LEN).unwrap(),
            SecretBuf::random(KEY_LEN).unwrap(),
            SecretBuf::random(KEY_LEN).unwrap(),
        );
        let m = |id: &str, kind| Method {
            id: id.into(),
            kind,
            label: id.into(),
            created_ms: NOW,
            kdf: None,
            salt: None,
        };
        let mk = vec![
            (m("passphrase", MethodKind::Passphrase), SecretBuf::copy_of(k1.as_slice()).unwrap()),
            (m("recovery", MethodKind::RecoveryKey), SecretBuf::copy_of(k2.as_slice()).unwrap()),
            (m("recovery-2", MethodKind::RecoveryKey), SecretBuf::copy_of(k3.as_slice()).unwrap()),
        ];
        let three = rebuild(&vmk, &mk, 3).expect("3-required over 3 soft methods");
        assert_eq!(three.envelopes.len(), 1, "C(3,3)");
        assert!(
            unlock(
                &three,
                &[
                    Factor::MethodSecret { id: "passphrase", secret: k1.as_slice() },
                    Factor::MethodSecret { id: "recovery", secret: k2.as_slice() },
                ],
            )
            .is_err(),
            "two of three is not enough at 3-required"
        );
        assert!(
            unlock(
                &three,
                &[
                    Factor::MethodSecret { id: "passphrase", secret: k1.as_slice() },
                    Factor::MethodSecret { id: "recovery", secret: k2.as_slice() },
                    Factor::MethodSecret { id: "recovery-2", secret: k3.as_slice() },
                ],
            )
            .is_ok()
        );
    }

    /// CD-40 acceptance 2 (never-brick): the passphrase and recovery key are
    /// not removable, the policy cannot exceed the enrolled count, and a
    /// hardware-only envelope set is refused wherever it appears.
    #[test]
    fn never_brick_rules_hold() {
        let nv = fresh();
        assert!(matches!(
            remove_method(&nv.file, &nv.vmk, "passphrase"),
            Err(VaultError::Policy(_))
        ));
        assert!(matches!(
            remove_method(&nv.file, &nv.vmk, "recovery"),
            Err(VaultError::Policy(_))
        ));
        assert!(matches!(set_required(&nv.file, &nv.vmk, 3), Err(VaultError::Policy(_))));
        assert!(matches!(set_required(&nv.file, &nv.vmk, 0), Err(VaultError::Policy(_))));

        // A hand-built pathological file: only a hardware envelope. The
        // invariant refuses it (load would too).
        let mut bad = nv.file.clone();
        bad.methods.push(Method {
            id: "passkey-dead".into(),
            kind: MethodKind::Passkey,
            label: "K".into(),
            created_ms: NOW,
            kdf: None,
            salt: None,
        });
        bad.escrows.push(Escrow {
            method_id: "passkey-dead".into(),
            nonce: bad.escrows[0].nonce.clone(),
            wrapped: bad.escrows[0].wrapped.clone(),
        });
        bad.envelopes = vec![Envelope {
            method_ids: vec!["passkey-dead".into()],
            nonce: bad.envelopes[0].nonce.clone(),
            wrapped: bad.envelopes[0].wrapped.clone(),
        }];
        assert!(matches!(assert_never_brick(&bad), Err(VaultError::Policy(_))));
    }

    /// Rotations re-wrap atomically: the old factor dies with the old file,
    /// the new one works, and the VMK (and thus the vault data) is unchanged.
    #[test]
    fn change_passphrase_and_regenerate_recovery_rotate_cleanly() {
        let nv = fresh();
        let changed =
            change_passphrase(&nv.file, &nv.vmk, b"a brand new passphrase", &TEST_KDF, NOW + 1)
                .expect("change passphrase");
        assert!(unlock(&changed, &[Factor::Passphrase(b"correct horse battery staple")]).is_err());
        let vmk = unlock(&changed, &[Factor::Passphrase(b"a brand new passphrase")]).unwrap();
        assert_eq!(vmk.as_slice(), nv.vmk.as_slice(), "the VMK never changes on re-wrap");

        // Too-short replacement is refused before any crypto runs.
        assert!(matches!(
            change_passphrase(&nv.file, &nv.vmk, b"short", &TEST_KDF, NOW),
            Err(VaultError::Policy(_))
        ));

        let (rotated, new_display) =
            regenerate_recovery(&changed, &nv.vmk, NOW + 2).expect("regenerate recovery");
        let old_rk = parse_recovery(&nv.recovery_display).unwrap();
        let new_rk = parse_recovery(&new_display).unwrap();
        assert!(unlock(&rotated, &[Factor::Recovery(old_rk.as_slice())]).is_err());
        assert!(unlock(&rotated, &[Factor::Recovery(new_rk.as_slice())]).is_ok());
    }

    // --- Sealed app state ----------------------------------------------------

    #[test]
    fn sealed_state_round_trips_and_fails_closed() {
        let nv = fresh();
        let sealed = seal_state(&nv.vmk, b"{\"identity_seed\":\"placeholder\"}").unwrap();
        assert_eq!(&sealed[..8], SEAL_MAGIC);
        let opened = open_state(&nv.vmk, &sealed).unwrap();
        assert_eq!(opened.as_slice(), b"{\"identity_seed\":\"placeholder\"}");

        // Tamper → refused.
        let mut bent = sealed.clone();
        *bent.last_mut().unwrap() ^= 0x01;
        assert_eq!(open_state(&nv.vmk, &bent).unwrap_err(), VaultError::Crypto);

        // Wrong key → refused.
        let other = SecretBuf::random(KEY_LEN).unwrap();
        assert_eq!(open_state(&other, &sealed).unwrap_err(), VaultError::Crypto);

        // Truncation / wrong magic → refused.
        assert_eq!(open_state(&nv.vmk, &sealed[..20]).unwrap_err(), VaultError::Crypto);
        let mut wrong_magic = sealed.clone();
        wrong_magic[0] ^= 0xff;
        assert_eq!(open_state(&nv.vmk, &wrong_magic).unwrap_err(), VaultError::Crypto);
    }

    // --- Persistence ---------------------------------------------------------

    /// Save → load round-trips through real JSON on disk, the loaded file
    /// still unlocks, and load refuses an unknown version or a brick-shaped
    /// file rather than booting into it.
    #[test]
    fn vault_file_save_load_round_trip_and_fail_closed() {
        let dir = std::env::temp_dir().join(format!("cdvault-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vault.json");

        let nv = fresh();
        nv.file.save_to(&path).expect("save");
        let loaded = VaultFile::load_from(&path).expect("load").expect("present");
        assert!(unlock(&loaded, &[Factor::Passphrase(b"correct horse battery staple")]).is_ok());

        // Absent file → Ok(None), the no-vault state.
        assert!(VaultFile::load_from(&dir.join("absent.json")).unwrap().is_none());

        // Unknown version → refused.
        let mut future = loaded.clone();
        future.version = 99;
        std::fs::write(&path, serde_json::to_string(&future).unwrap()).unwrap();
        assert!(matches!(VaultFile::load_from(&path), Err(VaultError::Format(_))));

        // A brick-shaped file (no recovery method) → refused at load.
        let mut brick = loaded.clone();
        brick.methods.retain(|m| m.kind != MethodKind::RecoveryKey);
        brick.envelopes.retain(|e| e.method_ids != vec!["recovery".to_string()]);
        brick.escrows.retain(|e| e.method_id != "recovery");
        std::fs::write(&path, serde_json::to_string(&brick).unwrap()).unwrap();
        assert!(matches!(VaultFile::load_from(&path), Err(VaultError::Policy(_))));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The vault file must never contain the recovery key, the passphrase, or
    /// any wrapping key in the clear. Serializing and scanning is a cheap
    /// tripwire for the classic serde mistake (a secret field slipping into a
    /// Serialize derive).
    #[test]
    fn vault_json_contains_no_plaintext_secrets() {
        let nv = fresh();
        let json = serde_json::to_string(&nv.file).unwrap();
        let compact = nv.recovery_display.replace('-', "");
        assert!(!json.contains(&nv.recovery_display));
        assert!(!json.contains(&compact));
        assert!(!json.contains("correct horse battery staple"));
        assert!(!json.contains(&hex_encode(nv.vmk.as_slice())));
    }

    /// The product Argon2id parameters are the documented RFC 9106 second
    /// recommendation — a regression guard against silent weakening.
    #[test]
    fn product_kdf_params_match_rfc_9106_second_recommendation() {
        assert_eq!(KdfParams::PRODUCT.m_cost_kib, 65536, "64 MiB");
        assert_eq!(KdfParams::PRODUCT.t_cost, 3);
        assert_eq!(KdfParams::PRODUCT.p_cost, 4);
    }

    // --- Stage 1b: host capture + runtime -----------------------------------

    /// SecretInput edits correctly across multi-byte characters and wipes on
    /// clear — the host-captured entry buffer behind the iron law.
    #[test]
    fn secret_input_edits_utf8_and_wipes() {
        let mut i = SecretInput::new().unwrap();
        i.push_str("pä");
        i.push_str("ss🙂");
        assert_eq!(i.chars(), 5);
        assert_eq!(i.as_slice(), "päss🙂".as_bytes());
        i.backspace(); // removes the whole 4-byte emoji
        assert_eq!(i.as_slice(), "päss".as_bytes());
        i.backspace();
        i.backspace();
        i.backspace(); // removes the 2-byte ä
        assert_eq!(i.as_slice(), b"p");
        i.backspace();
        i.backspace(); // underflow is a no-op
        assert_eq!(i.chars(), 0);
        i.push_str("secret\n"); // control chars never enter a secret
        assert_eq!(i.as_slice(), b"secret");
        i.clear();
        assert_eq!(i.chars(), 0);
        assert!(i.buf.as_slice().iter().all(|&b| b == 0), "clear wipes the tail");
    }

    /// Poll a worker outcome (the workers run real threads).
    fn wait_outcome() -> Outcome {
        for _ in 0..500 {
            if let Some(o) = take_outcome() {
                return o;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("vault worker outcome never arrived");
    }

    /// The full Stage-1b runtime flow, end to end against a temp dir: setup
    /// through the host-capture state machine (mismatch retry included), the
    /// one-time recovery display + ack, a sealed tenant surviving a simulated
    /// relaunch, a failed then successful passphrase unlock, and a recovery-key
    /// unlock. ONE test drives the whole flow because the runtime is a
    /// process-wide singleton — two parallel tests would interleave.
    #[test]
    fn runtime_setup_lock_unlock_flow() {
        let dir = std::env::temp_dir().join(format!("cdvault-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        test_reset_runtime(&dir, TEST_KDF);

        assert!(!has_vault());
        assert!(!is_locked(), "no vault → the gate is open");
        assert!(begin_capture("unlock_pass").is_err(), "nothing to unlock yet");

        // Setup: first entry, a mismatching confirm (retry), then a match.
        begin_capture("setup_pass").unwrap();
        key_text("correct horse battery staple");
        key_submit(); // → confirm step
        key_text("correct horse battery stapl"); // typo
        key_submit();
        assert!(
            state_json().contains("do not match"),
            "mismatching confirm restarts setup with an error"
        );
        key_text("correct horse battery staple");
        key_submit();
        key_text("correct horse battery staple");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::SetupDone);
        assert!(is_unlocked(), "setup leaves the session unlocked");
        assert!(dir.join("vault.json").exists());

        // The one-time recovery display: present until acked, then gone.
        let state: serde_json::Value = serde_json::from_str(&state_json()).unwrap();
        let recovery = state["recovery"].as_str().expect("recovery shown once").to_string();
        assert_eq!(recovery.split('-').count(), 14);
        setup_ack();
        assert!(state_json().contains("\"recovery\":null"));

        // A sealed tenant written while unlocked…
        sealed_set("identity_seed", "00aa11bb");
        assert_eq!(sealed_get("identity_seed").as_deref(), Some("00aa11bb"));

        // …survives a relaunch (runtime reset), unreadable while locked.
        test_reset_runtime(&dir, TEST_KDF);
        assert!(is_locked(), "vault present + no VMK → gate closed");
        assert_eq!(sealed_get("identity_seed"), None, "sealed stays sealed while locked");

        // Wrong passphrase fails (uniform error), correct one opens the gate.
        begin_capture("unlock_pass").unwrap();
        key_text("wrong horse battery staple");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::UnlockFailed);
        assert!(is_locked());
        key_text("correct horse battery staple");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::Unlocked);
        assert!(is_unlocked());
        assert_eq!(
            sealed_get("identity_seed").as_deref(),
            Some("00aa11bb"),
            "the sealed tenant is back after unlock"
        );

        // Recovery-key unlock, via the same host-captured entry (typed with
        // dashes, exactly as printed).
        test_reset_runtime(&dir, TEST_KDF);
        begin_capture("unlock_recovery").unwrap();
        key_text(&recovery);
        key_submit();
        assert_eq!(wait_outcome(), Outcome::Unlocked);
        assert!(is_unlocked(), "the printed recovery key unlocks independently");

        // Lock request queues a relaunch; wipe drops every secret.
        request_lock();
        assert!(take_relaunch());
        assert!(!take_relaunch(), "one-shot");
        wipe_for_exit();
        assert!(!is_unlocked());

        std::fs::remove_dir_all(&dir).ok();
    }
}
