//! CyberDesk Vault - crypto core. CD-40 (D-0058) built the envelope layer;
//! CD-42 (D-0062) set the authoritative unlock model.
//!
//! Envelope key management: one random 256-bit **Vault Master Key (VMK)**
//! protects the vault's sensitive data; the VMK itself is never derived from
//! any single factor. Independent **envelopes** each wrap the VMK - enrolling
//! or removing an unlock method re-wraps the VMK, never re-encrypts the
//! protected data.
//!
//! ## The authoritative unlock model (CD-42, D-0062)
//!
//! * **Master password** - mandatory at first launch, the sole root. Argon2id
//!   (explicit tuned params, stored per method) → 32-byte wrapping key.
//!   Always enrolled, never removable, always a required factor.
//! * **Passkey (WebAuthn PRF)** - the only optional additional factor (at
//!   most one). The PRF-derived secret is the wrapping key; the core treats
//!   it as an opaque 32-byte method secret (the WebAuthn layer follows the
//!   D-0061 go-live gate).
//! * **No recovery key, no backdoor.** The master password is the sole
//!   1-factor recovery. A forgotten master password - or a lost passkey while
//!   2FA is required - makes the vault unrecoverable, by design.
//!
//! ## Unlock policy is structural, not a checked flag
//!
//! The policy is exactly one of two shapes, enforced by **which envelopes
//! exist**: password-only has the single envelope `{password}`; password +
//! passkey (2FA) has the single envelope `{password, passkey}`, wrapped by a
//! key combined (BLAKE2s, domain-separated) from both methods' wrapping keys.
//! A passkey alone can never open anything - no envelope exists that does not
//! include the master password - and editing the `required` field in
//! `vault.json` changes nothing, because the mutable field is UI metadata;
//! the cryptography is in the envelope set.
//!
//! ## Escrows make re-wrapping possible from an unlocked session
//!
//! Every method's wrapping key is also stored wrapped **under the VMK** (an
//! "escrow"). Enrolling a passkey / changing the policy from an unlocked
//! session needs every method's wrapping key to build the new envelope set -
//! the escrows provide them without re-prompting for each factor. This adds
//! nothing an attacker could use: whoever holds the VMK has already won the
//! current vault (the escrows are decryptable only *with* the VMK), and
//! rotating a method replaces its escrow. Recorded in D-0058.
//!
//! ## Structural invariants
//!
//! Exactly one master-password method, at most one passkey, every envelope
//! includes the password, and the envelope set matches the policy shape.
//! [`VaultFile::load_from`] refuses a file violating the invariants, so a
//! hand-edited or corrupted vault fails closed instead of booby-trapping a
//! later unlock. (The CD-40 "never-brick" rule - a mandatory non-hardware
//! fallback - was deliberately retired with the recovery key: under 2FA a
//! lost passkey bricks the vault BY DESIGN, D-0062.)
//!
//! ## Memory hygiene (closes the CD-33-deferred Tasks C/D for vault keys)
//!
//! All key material lives in [`SecretBuf`]s: dedicated `VirtualAlloc`ed pages,
//! `VirtualLock`ed out of the pagefile, zeroized then unlocked and released on
//! drop. Allocation **fails closed** - if the pages cannot be locked (after
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
//! erasure (`zeroize`) - all pinned, license-checked (D-0005), verified at
//! source against the exact crate versions.

// The passkey half of the model (enroll/unlock plumbing) waits on the D-0061
// WebAuthn go-live gate; its core seams are kept warm here. Mirrors the
// store.rs precedent.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use argon2::{Algorithm, Argon2, Block, Params, Version};
use blake2::{Blake2s256, Digest};
use chacha20poly1305::{
    Key, Tag, XChaCha20Poly1305, XNonce,
    aead::{AeadInPlace, KeyInit},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

/// Key sizes. Everything is 256-bit: the VMK, every wrapping key, and the
/// combined envelope keys.
pub const KEY_LEN: usize = 32;
/// XChaCha20-Poly1305 extended nonce (24 bytes - safe to draw at random per
/// wrap; the birthday bound on 192 bits is unreachable).
const NONCE_LEN: usize = 24;
/// Poly1305 authentication tag.
const TAG_LEN: usize = 16;
/// Per-passphrase-envelope Argon2 salt (16 random bytes; crate minimum is 8).
const SALT_LEN: usize = 16;
/// Minimum master-password length in bytes (NIST SP 800-63B's floor for
/// user-chosen secrets; the strength meter targets far higher, but the
/// informed-override stops here - below 8 there is no vault to speak of).
pub const MIN_PASSPHRASE_LEN: usize = 8;
/// The strength meter's "weak" line: zxcvbn's own documentation says any
/// score below 3 should be considered too weak. Submitting below it stages
/// the informed override (CD-42 Task B) - never a hard block.
const WEAK_SCORE_FLOOR: u8 = 3;
/// The meter's length criterion ("very complex" target - advisory, shown as
/// a met/unmet criterion, never enforced).
const TARGET_LEN: usize = 12;
/// The strength estimator evaluates at most this many leading characters.
/// zxcvbn's matching is superlinear in input length and runs per keystroke
/// on the UI thread; 64 characters are far past every target, and a capped
/// score can only UNDER-state the full secret's strength.
const STRENGTH_EVAL_CAP: usize = 64;
/// The vault file format version this build reads and writes. Version 1 was
/// the CD-40 recovery-key model - retired by D-0062; v1 files are refused
/// with a reset message (dev data only, sanctioned by the CD-42 briefing).
const VAULT_VERSION: u32 = 2;

/// AEAD domain separation. Every wrap context has its own associated data, so
/// a blob can never be replayed in a different role (an envelope is not an
/// escrow is not a sealed-state blob), and an envelope is bound to the exact
/// method set that keys it.
const AAD_ENVELOPE: &str = "cyberdesk.vault.v1.envelope:";
const AAD_ESCROW: &str = "cyberdesk.vault.v1.escrow:";
/// Sealed app-state container magic - also its AAD.
const SEAL_MAGIC: &[u8; 8] = b"CDSEAL01";
/// Domain prefix for combining method wrapping keys into an envelope key.
/// Single-method envelopes run through the same PRF (uniform code path and
/// domain separation of raw method secrets from envelope keys).
const COMBINE_DOMAIN: &[u8] = b"cyberdesk.vault.v1.combine";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Vault errors. `Crypto` is deliberately uniform - a wrong passphrase, a
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
// SecretBuf - locked, zeroized key memory
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

/// Non-Windows fallback (dev/CI portability only - the product target is
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

    /// A locked duplicate (locked → locked copy) - hands a worker thread its
    /// own VMK copy while the runtime keeps the session's.
    pub fn try_clone(&self) -> Result<Self> {
        Self::copy_of(self.as_slice())
    }

    /// A fresh CSPRNG-filled locked buffer (VMK, recovery key, salts live
    /// elsewhere - this is for keys).
    pub fn random(len: usize) -> Result<Self> {
        let mut b = Self::zeroed(len)?;
        getrandom::fill(b.as_mut_slice())
            .map_err(|e| VaultError::Kdf(format!("csprng failed: {e}")))?;
        Ok(b)
    }
}

/// Redacted - key material must never reach a log line.
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
/// is RFC 9106's second recommended configuration (64 MiB, t=3, p=4) - the
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

/// What kind of factor a method is. Exactly two exist (D-0062): the master
/// password (mandatory root, `id = "passphrase"` for file stability) and the
/// optional passkey. `hardware()` marks the device-bound one - the only
/// removable kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MethodKind {
    Passphrase,
    Passkey,
}

impl MethodKind {
    pub fn hardware(self) -> bool {
        matches!(self, MethodKind::Passkey)
    }
}

/// One enrolled unlock method. The wrapping key itself is never stored here -
/// it is re-derived at unlock (master password), re-presented (passkey PRF),
/// or recovered from its escrow (mutations from an unlocked session).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Method {
    /// Stable id: `"passphrase"`, `"passkey-<hex>"`. Envelope membership and
    /// escrows reference methods by id.
    pub id: String,
    pub kind: MethodKind,
    /// User-facing label for the config surface ("Passphrase", "YubiKey 5"…).
    pub label: String,
    /// Mint time (unix epoch ms) for honest status display.
    pub created_ms: u64,
    /// Passphrase methods only: the Argon2id cost parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kdf: Option<KdfParams>,
    /// The per-method random salt, hex. For the passphrase it feeds Argon2id;
    /// for the passkey it is the PRF eval value the OS converts per the
    /// WebAuthn spec (CD-43, D-0063). Non-secret either way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    /// Passkey methods only: the WebAuthn credential id (hex, non-secret) -
    /// the allowlist entry for the unlock-time Hello assertion (CD-43).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cred_id: Option<String>,
}

/// One VMK envelope: the VMK wrapped by the combined key of `method_ids`
/// (sorted; `{passphrase}` at password-only, `{passkey-…, passphrase}` at
/// 2FA). `wrapped` is ciphertext ‖ tag, hex; the AAD binds the exact method
/// set.
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
/// `required` is UI metadata - the policy itself is structural (see module
/// docs); [`VaultFile::load_from`] re-validates every invariant on read.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct VaultFile {
    pub version: u32,
    /// 1 = password-only, 2 = password + passkey (2FA). UI metadata mirroring
    /// the envelope shape - the policy itself is structural.
    pub required: u8,
    pub methods: Vec<Method>,
    pub envelopes: Vec<Envelope>,
    pub escrows: Vec<Escrow>,
}

/// The result of [`create`]: the fresh vault file and the live VMK.
pub struct NewVault {
    pub file: VaultFile,
    pub vmk: SecretBuf,
}

/// A factor presented at unlock time. Secrets are borrowed - the caller keeps
/// them in locked memory ([`SecretBuf`]) and drops them right after.
pub enum Factor<'a> {
    /// The master password, raw bytes (resolved to the enrolled method).
    Passphrase(&'a [u8]),
    /// Any method by id with its raw 32-byte secret (passkey PRF output).
    MethodSecret { id: &'a str, secret: &'a [u8] },
}

// ---------------------------------------------------------------------------
// Crypto primitives (thin, on vetted crates - no custom constructions)
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
    blob.extend_from_slice(work.as_slice()); // ciphertext - public
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
// Envelope-set construction (the policy lives here)
// ---------------------------------------------------------------------------

/// Build the full vault file from the VMK, every enrolled method WITH its
/// wrapping key, and the policy. This is the single place envelopes and
/// escrows are minted - every mutation (enroll, remove, policy change,
/// rotation) funnels through here, and the structural invariants are enforced
/// before anything is returned. The policy is exactly one of two shapes
/// (D-0062): `required = 1` mints the single envelope `{password}`;
/// `required = 2` mints the single envelope `{password, passkey}` - the
/// master password is a member of EVERY envelope, so a passkey alone can
/// never open anything.
fn rebuild(
    vmk: &SecretBuf,
    methods_keys: &[(Method, SecretBuf)],
    required: u8,
) -> Result<VaultFile> {
    // Deterministic order everywhere: methods sorted by id; an envelope's
    // member list and its key-combination order follow the same sort.
    let mut mk: Vec<&(Method, SecretBuf)> = methods_keys.iter().collect();
    mk.sort_by(|a, b| a.0.id.cmp(&b.0.id));
    if mk.windows(2).any(|w| w[0].0.id == w[1].0.id) {
        return Err(VaultError::Format("duplicate method id".into()));
    }

    let pw = mk
        .iter()
        .position(|m| m.0.kind == MethodKind::Passphrase)
        .ok_or_else(|| VaultError::Policy("no master-password method".into()))?;
    let combo: Vec<usize> = match required {
        // Password-only: the password's envelope, nothing else.
        1 => vec![pw],
        // 2FA: password + passkey together - both, or it's a policy error.
        2 => {
            if !mk.iter().any(|m| m.0.kind == MethodKind::Passkey) {
                return Err(VaultError::Policy(
                    "two-factor unlock requires an enrolled passkey".into(),
                ));
            }
            (0..mk.len()).collect()
        }
        other => {
            return Err(VaultError::Policy(format!(
                "policy must be password-only (1) or password + passkey (2), got {other}"
            )));
        }
    };

    let ids: Vec<String> = combo.iter().map(|&i| mk[i].0.id.clone()).collect();
    let keys: Vec<&SecretBuf> = combo.iter().map(|&i| &mk[i].1).collect();
    let combined = combine_keys(&keys)?;
    let (nonce, blob) = aead_wrap(&combined, &envelope_aad(&ids), vmk)?;
    let envelopes = vec![Envelope {
        method_ids: ids,
        nonce: hex_encode(&nonce),
        wrapped: hex_encode(&blob),
    }];

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
    assert_model(&file)?;
    Ok(file)
}

/// The structural invariants of the D-0062 model: exactly one master-password
/// method, at most one passkey, no other kinds; the policy is 1 or 2; exactly
/// one envelope whose member set matches the policy shape - and the password
/// is a member of it (a passkey alone can never unlock); every method has
/// exactly one escrow. Checked on every rebuild and on every load, so a
/// violating (hand-edited, corrupted) file is refused before it can lie about
/// its own policy.
pub fn assert_model(file: &VaultFile) -> Result<()> {
    let pw_count = file.methods.iter().filter(|m| m.kind == MethodKind::Passphrase).count();
    if pw_count != 1 {
        return Err(VaultError::Policy(format!(
            "expected exactly one master-password method, found {pw_count}"
        )));
    }
    let pk_count = file.methods.iter().filter(|m| m.kind == MethodKind::Passkey).count();
    if pk_count > 1 {
        return Err(VaultError::Policy(format!(
            "at most one passkey may be enrolled, found {pk_count}"
        )));
    }
    let pw_id = &file
        .methods
        .iter()
        .find(|m| m.kind == MethodKind::Passphrase)
        .expect("counted above")
        .id;
    let expected_members: Vec<&String> = match file.required {
        1 => vec![pw_id],
        2 => {
            if pk_count == 0 {
                return Err(VaultError::Policy(
                    "two-factor policy with no passkey enrolled".into(),
                ));
            }
            // Every enrolled method, in the id-sorted order rebuild() uses.
            file.methods.iter().map(|m| &m.id).collect()
        }
        other => {
            return Err(VaultError::Policy(format!(
                "policy must be password-only (1) or password + passkey (2), got {other}"
            )));
        }
    };
    let [envelope] = file.envelopes.as_slice() else {
        return Err(VaultError::Policy(format!(
            "expected exactly one envelope, found {}",
            file.envelopes.len()
        )));
    };
    let members: Vec<&String> = envelope.method_ids.iter().collect();
    if members != expected_members {
        return Err(VaultError::Policy(
            "the envelope's method set does not match the unlock policy".into(),
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

/// Set up a fresh vault: generate the VMK and enroll the master password
/// (Argon2id, `kdf`) - the sole mandatory root (D-0062). The policy starts at
/// password-only; a passkey may join later as the only additional factor.
pub fn create(passphrase: &[u8], kdf: &KdfParams, now_ms: u64) -> Result<NewVault> {
    if passphrase.len() < MIN_PASSPHRASE_LEN {
        return Err(VaultError::Policy(format!(
            "the master password must be at least {MIN_PASSPHRASE_LEN} bytes"
        )));
    }
    let vmk = SecretBuf::random(KEY_LEN)?;
    let salt: [u8; SALT_LEN] = rand_array()?;
    let pp_key = derive_passphrase_key(passphrase, &salt, kdf)?;

    let methods_keys = vec![(
        Method {
            id: "passphrase".into(),
            kind: MethodKind::Passphrase,
            label: "Master password".into(),
            created_ms: now_ms,
            kdf: Some(*kdf),
            salt: Some(hex_encode(&salt)),
            cred_id: None,
        },
        pp_key,
    )];
    let file = rebuild(&vmk, &methods_keys, 1)?;
    Ok(NewVault { file, vmk })
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
/// needs no checking here - at 2FA only the pair envelope exists, so a single
/// factor finds no candidate and CANNOT open anything, whatever the mutable
/// `required` field claims. Failure is uniform ([`VaultError::Crypto`]).
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

/// Enroll THE passkey (at most one, D-0062) from an unlocked session. At
/// password-only the envelope set is unchanged - the passkey gains an escrow
/// so a later switch to 2FA can mint the pair envelope; it opens nothing on
/// its own. Re-wraps the VMK; the vault data is untouched. `cred_id` and
/// `prf_salt` (both hex, non-secret) are the Hello assertion inputs (CD-43);
/// a mock/test enrollment passes None.
pub fn enroll_passkey(
    file: &VaultFile,
    vmk: &SecretBuf,
    label: &str,
    secret: &[u8],
    cred_id: Option<String>,
    prf_salt: Option<String>,
    now_ms: u64,
) -> Result<VaultFile> {
    if secret.len() != KEY_LEN {
        return Err(VaultError::Format("passkey secret must be 32 bytes".into()));
    }
    if file.methods.iter().any(|m| m.kind == MethodKind::Passkey) {
        return Err(VaultError::Policy(
            "a passkey is already enrolled - remove it before adding another".into(),
        ));
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
            salt: prf_salt,
            cred_id,
        },
        SecretBuf::copy_of(secret)?,
    ));
    rebuild(vmk, &mk, file.required)
}

/// Remove an enrolled method. Only the passkey is removable - the master
/// password is the mandatory root - and not while the 2FA policy requires it
/// (switch to password-only first; that weakening carries its own confirm
/// gate, so the policy step is never silently skipped).
pub fn remove_method(file: &VaultFile, vmk: &SecretBuf, id: &str) -> Result<VaultFile> {
    let m = file
        .methods
        .iter()
        .find(|m| m.id == id)
        .ok_or_else(|| VaultError::Format(format!("unknown method '{id}'")))?;
    if !m.kind.hardware() {
        return Err(VaultError::Policy("the master password cannot be removed".into()));
    }
    if file.required >= 2 {
        return Err(VaultError::Policy(
            "the passkey is required by the two-factor policy - switch to password-only first"
                .into(),
        ));
    }
    let mk: Vec<(Method, SecretBuf)> = unwrap_all_escrows(file, vmk)?
        .into_iter()
        .filter(|(m, _)| m.id != id)
        .collect();
    rebuild(vmk, &mk, file.required)
}

/// Change the unlock policy: 1 = password-only, 2 = password + passkey (2FA).
/// Structural: mints the new envelope shape (2 needs an enrolled passkey).
pub fn set_required(file: &VaultFile, vmk: &SecretBuf, required: u8) -> Result<VaultFile> {
    let mk = unwrap_all_escrows(file, vmk)?;
    rebuild(vmk, &mk, required)
}

/// Change the master password: fresh salt, fresh Argon2id derivation with
/// `kdf` (also the path for re-tuning the cost parameters), full re-wrap. The
/// old password stops working atomically with the new file.
pub fn change_passphrase(
    file: &VaultFile,
    vmk: &SecretBuf,
    new_passphrase: &[u8],
    kdf: &KdfParams,
    now_ms: u64,
) -> Result<VaultFile> {
    if new_passphrase.len() < MIN_PASSPHRASE_LEN {
        return Err(VaultError::Policy(format!(
            "the master password must be at least {MIN_PASSPHRASE_LEN} bytes"
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

/// `vault.json` - envelope metadata (public by design; see [`VaultFile`]).
pub fn vault_file_path() -> PathBuf {
    crate::store::data_dir().join("vault.json")
}

/// `vault.seal` - the sealed sensitive app state (Stage 1b wires tenants in).
pub fn sealed_state_path() -> PathBuf {
    crate::store::data_dir().join("vault.seal")
}

impl VaultFile {
    /// Load and validate a vault file. `Ok(None)` when none exists (no vault
    /// set up). A file that parses but violates the format version or the
    /// never-brick invariant is refused - fail closed, never boot into a
    /// state that could strand the user deeper in.
    pub fn load_from(path: &Path) -> Result<Option<VaultFile>> {
        let raw = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(VaultError::Io(format!("read {}: {e}", path.display()))),
        };
        // Version first, on the raw JSON: a v1 file (the retired CD-40
        // recovery-key model) must produce the reset message, not a serde
        // error about a method kind this build no longer knows.
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|e| VaultError::Format(format!("vault.json: {e}")))?;
        match value.get("version").and_then(|v| v.as_u64()) {
            Some(1) => {
                return Err(VaultError::Format(
                    "this vault uses the retired recovery-key model (pre-CD-42). \
                     There is no migration: delete vault.json and vault.seal from the \
                     CyberDesk data directory and set up a new master password."
                        .into(),
                ));
            }
            Some(v) if v == VAULT_VERSION as u64 => {}
            v => {
                return Err(VaultError::Format(format!(
                    "unsupported vault version {v:?} (this build reads {VAULT_VERSION})"
                )));
            }
        }
        let file: VaultFile = serde_json::from_value(value)
            .map_err(|e| VaultError::Format(format!("vault.json: {e}")))?;
        assert_model(&file)?;
        Ok(Some(file))
    }

    /// Atomic save: write a sibling temp file, then rename over the target
    /// (`std::fs::rename` replaces on Windows). A crash mid-write leaves the
    /// previous consistent file in place - key management must never be
    /// half-written.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        assert_model(self)?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| VaultError::Format(format!("serialize: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &json).map_err(|e| VaultError::Io(format!("write tmp: {e}")))?;
        std::fs::rename(&tmp, path).map_err(|e| VaultError::Io(format!("rename: {e}")))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SecretInput - host-captured secret entry (Stage 1b)
// ---------------------------------------------------------------------------

/// A typed-in secret being assembled in locked memory. This is the iron-law
/// mechanism: while the vault captures input, the HOST consumes the window's
/// key events and appends them here - the lock/settings page never receives a
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
// to COMMIT results - the Argon2 derivation itself runs without any lock held
// (CD-38 threading law: nothing here is ever awaited on the router's dispatch
// stack, and no vault lock is held across a CEF call).

/// What the host is currently capturing keystrokes for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CaptureKind {
    /// The master password, to unlock. (Under 2FA the passkey assertion joins
    /// as a host-driven WebAuthn step after the password - D-0061 go-live.)
    UnlockPass,
    /// The new master password during first-launch vault setup…
    SetupPass,
    /// …and its confirmation re-type (internal step, never begun via IPC).
    SetupConfirm,
    /// The replacement master password (change flow, unlocked session)…
    ChangePass,
    /// …and its confirmation re-type (internal step).
    ChangeConfirm,
    /// The CURRENT master password, to authorize an Argon2id cost re-tune
    /// (the new params are staged in `pending_kdf`; the entry is verified
    /// against the existing envelope before anything is re-derived).
    RetuneKdf,
}

impl CaptureKind {
    fn as_str(self) -> &'static str {
        match self {
            CaptureKind::UnlockPass => "unlock_pass",
            CaptureKind::SetupPass => "setup_pass",
            CaptureKind::SetupConfirm => "setup_confirm",
            CaptureKind::ChangePass => "change_pass",
            CaptureKind::ChangeConfirm => "change_confirm",
            CaptureKind::RetuneKdf => "retune_kdf",
        }
    }
}

/// A host-computed strength snapshot of the password being typed (CD-42
/// Task B, D-0062). This - and ONLY this - crosses to the renderer: a coarse
/// score, a met/unmet length criterion and zxcvbn's canned feedback strings
/// (fixed enum texts that never echo input). The password characters stay in
/// host-locked memory; the meter is honest without breaking the iron law.
#[derive(Clone, Debug)]
struct Strength {
    /// zxcvbn score 0..=4 (the crate's own scale; < 3 is "too weak").
    score: u8,
    /// Characters typed (the length criterion is chars, not bytes).
    chars: usize,
    /// zxcvbn's canned warning, if any (set only at score <= 2).
    warning: Option<String>,
    /// zxcvbn's canned improvement suggestions.
    suggestions: Vec<String>,
}

/// Evaluate the typed secret with the vetted `zxcvbn` estimator (MIT,
/// license-checked per D-0005 - no hand-rolled strength rules). Runs in the
/// HOST only. Bounded residual (documented in cyberdesk-security.md): the
/// estimator processes a transient copy of the password in regular heap
/// memory during evaluation - same tier as the crypto crates' internal
/// state; the copy this function owns is zeroized before returning.
fn eval_strength(input: &SecretInput) -> Strength {
    let s = input.as_str();
    let chars = s.chars().count();
    let mut capped: String = s.chars().take(STRENGTH_EVAL_CAP).collect();
    let entropy = zxcvbn::zxcvbn(&capped, &[]);
    capped.zeroize();
    let feedback = entropy.feedback();
    Strength {
        score: entropy.score().into(),
        chars,
        warning: feedback.and_then(|f| f.warning()).map(|w| w.to_string()),
        suggestions: feedback
            .map(|f| f.suggestions().iter().map(|s| s.to_string()).collect())
            .unwrap_or_default(),
    }
}

/// A finished background operation, taken by the shell (`about_to_wait`) to
/// drive the UI transition. The VMK itself never rides an outcome - the worker
/// commits it straight into the runtime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The vault unlocked - boot the workspace.
    Unlocked,
    /// An unlock attempt failed (the uniform error is already in the state).
    UnlockFailed,
    /// Setup finished - the vault exists and the session is unlocked.
    SetupDone,
    /// Setup failed (error in the state).
    SetupFailed,
    /// A re-wrap finished (master-password change / KDF re-tune).
    Rewrapped,
    /// A re-wrap failed (error in the state).
    RewrapFailed,
}

struct Runtime {
    /// Directory holding `vault.json` / `vault.seal` (the app-data dir; tests
    /// point it at a temp dir).
    dir: PathBuf,
    file: Option<VaultFile>,
    /// A vault file exists but could not be validated (tamper/corruption).
    /// Fail-closed: the gate stays locked and unlock cannot succeed - booting
    /// as "no vault" on a broken file would let corruption bypass the gate.
    broken: Option<String>,
    vmk: Option<SecretBuf>,
    /// Dev bypass engaged (debug builds only): the gate is skipped, the
    /// sealed state stays sealed.
    bypassed: bool,
    capture: Option<CaptureKind>,
    input: Option<SecretInput>,
    /// The first entry held between a setup/change step and its confirm
    /// re-type.
    pending_pass: Option<SecretInput>,
    /// Live strength snapshot of the entry - present only while a NEW master
    /// password is being typed (setup/change), recomputed per keystroke.
    strength: Option<Strength>,
    /// A weak entry was submitted: the flow is parked on the prominent
    /// warning until the page's explicit "use it anyway" IPC (the informed
    /// override) - Enter never overrides, and further typing re-evaluates.
    weak_pending: bool,
    /// A Windows Hello modal is up on a worker: `"enroll"` (passkey
    /// enrollment, two prompts) or `"assert"` (the 2FA unlock step). Drives
    /// the pages' "follow the Windows Hello prompt" hint (CD-43).
    hello: Option<&'static str>,
    /// First launch just created the vault and the passkey offer is on
    /// screen (CD-44 D1). Purely a UI step: the vault is already usable, the
    /// offer is declinable, and declining continues into the workspace.
    offer_passkey: bool,
    busy: bool,
    error: Option<String>,
    outcome: Option<Outcome>,
    relaunch: bool,
    /// The decrypted sealed app state (`vault.seal`), present only while
    /// unlocked. JSON object; tenants: identity_seed, identity_seed_born.
    sealed: Option<serde_json::Value>,
    /// KDF cost for setup (product default; tests override).
    kdf: KdfParams,
    /// New Argon2id params staged by a cost re-tune, applied once the current
    /// passphrase is captured and verified (CD-40 1c).
    pending_kdf: Option<KdfParams>,
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
            strength: None,
            weak_pending: false,
            hello: None,
            offer_passkey: false,
            busy: false,
            error: None,
            outcome: None,
            relaunch: false,
            sealed: None,
            kdf: KdfParams::PRODUCT,
            pending_kdf: None,
        })
    })
}

/// Does this capture type a NEW master password (the meter's home)?
fn captures_new_password(kind: Option<CaptureKind>) -> bool {
    matches!(kind, Some(CaptureKind::SetupPass) | Some(CaptureKind::ChangePass))
}

// --- Windows Hello bridge (CD-43, D-0063) -----------------------------------

/// The shell's top-level window handle, registered once at boot - the Hello
/// modal's parent. Workers read it without the runtime lock.
static SHELL_HWND: AtomicIsize = AtomicIsize::new(0);

pub fn set_shell_hwnd(hwnd: isize) {
    SHELL_HWND.store(hwnd, Ordering::Relaxed);
}

fn shell_hwnd() -> isize {
    SHELL_HWND.load(Ordering::Relaxed)
}

/// The unit suite must never pop a Hello prompt: under `cfg(test)` the
/// platform seam answers from this injectable mock instead of webauthn.dll.
/// `None` = the mock "platform" is unavailable.
#[cfg(test)]
fn test_prf() -> &'static Mutex<Option<Vec<u8>>> {
    use std::sync::OnceLock;
    static PRF: OnceLock<Mutex<Option<Vec<u8>>>> = OnceLock::new();
    PRF.get_or_init(|| Mutex::new(None))
}

/// The credential id the test mock "mints" - asserted back at unlock so the
/// cred-id plumbing is exercised end to end.
#[cfg(test)]
const TEST_CRED_ID: &[u8] = b"mock-hello-credential";

/// Platform enroll: create the Hello credential and derive the PRF secret
/// (two modal prompts - see webauthn.rs). Returns (credential id, secret).
fn platform_enroll(hwnd: isize, salt: &[u8; KEY_LEN]) -> std::result::Result<(Vec<u8>, SecretBuf), String> {
    #[cfg(test)]
    {
        let _ = (hwnd, salt);
        return match test_prf().lock().unwrap().as_ref() {
            Some(s) => Ok((
                TEST_CRED_ID.to_vec(),
                SecretBuf::copy_of(s).map_err(|e| e.to_string())?,
            )),
            None => Err("mock platform unavailable".into()),
        };
    }
    #[cfg(all(not(test), windows))]
    {
        let e = crate::webauthn::enroll(hwnd, salt).map_err(|e| e.to_string())?;
        Ok((e.cred_id, e.secret))
    }
    #[cfg(all(not(test), not(windows)))]
    {
        let _ = (hwnd, salt);
        Err("Windows Hello is unavailable on this platform".into())
    }
}

/// Platform assert: the unlock-time Hello step - one modal prompt, returns
/// the PRF-derived method secret for the stored (cred id, salt).
fn platform_assert(
    hwnd: isize,
    cred_id: &[u8],
    salt: &[u8; KEY_LEN],
) -> std::result::Result<SecretBuf, String> {
    #[cfg(test)]
    {
        let _ = (hwnd, salt);
        if cred_id != TEST_CRED_ID {
            return Err("mock: unknown credential id".into());
        }
        return match test_prf().lock().unwrap().as_ref() {
            Some(s) => SecretBuf::copy_of(s).map_err(|e| e.to_string()),
            None => Err("mock platform unavailable".into()),
        };
    }
    #[cfg(all(not(test), windows))]
    {
        crate::webauthn::assert(hwnd, cred_id, salt).map_err(|e| e.to_string())
    }
    #[cfg(all(not(test), not(windows)))]
    {
        let _ = (hwnd, cred_id, salt);
        Err("Windows Hello is unavailable on this platform".into())
    }
}

/// The platform capability snapshot for the honest config surface:
/// (DLL available, webauthn.dll API version, Windows Hello set up). The
/// Hello flag is a live machine fact re-probed per push, so the surface
/// updates itself once a PIN/fingerprint/face is enrolled (CD-44 A3).
fn platform_info() -> (bool, u32, bool) {
    #[cfg(test)]
    {
        let mocked = test_prf().lock().unwrap().is_some();
        (mocked, 0, mocked)
    }
    #[cfg(all(not(test), windows))]
    {
        (
            crate::webauthn::available(),
            crate::webauthn::api_version(),
            crate::webauthn::hello_ready(),
        )
    }
    #[cfg(all(not(test), not(windows)))]
    {
        (false, 0, false)
    }
}

/// Recompute (or drop) the strength snapshot to match the current capture.
fn refresh_strength(r: &mut Runtime) {
    r.strength = if captures_new_password(r.capture) {
        r.input.as_ref().map(eval_strength)
    } else {
        None
    };
}

/// Load the vault state at boot (after `settings::init`, before any view).
/// With a valid vault present the app starts LOCKED; with NO vault it starts
/// in mandatory first-launch setup (CD-42, D-0062) - either way the gate is
/// closed until a worker outcome opens it. The dev bypass
/// (`CYBERDESK_VAULT_BYPASS=1`) exists ONLY in debug builds - the check is
/// `cfg(debug_assertions)`-gated, so a release artifact contains no bypass
/// code path at all; it skips the GATE (unlock or mandatory setup), never
/// the cryptography (the sealed state stays sealed - the VMK cannot be
/// conjured).
pub fn init() {
    let mut r = rt().lock().unwrap();
    let path = r.dir.join("vault.json");
    match VaultFile::load_from(&path) {
        Ok(file) => r.file = file,
        Err(e) => {
            tracing::error!("vault.json failed to load - staying locked: {e}");
            r.broken = Some(e.to_string());
        }
    }
    #[cfg(debug_assertions)]
    if std::env::var("CYBERDESK_VAULT_BYPASS").as_deref() == Ok("1") {
        tracing::warn!(
            "VAULT DEV BYPASS ACTIVE (debug build): gate skipped, sealed state stays sealed"
        );
        r.bypassed = true;
    }
    if !r.bypassed {
        if r.file.is_some() {
            tracing::info!("vault present - starting locked");
        } else if r.broken.is_none() {
            tracing::info!("no vault - starting in mandatory first-launch setup");
        }
    }
}

pub fn has_vault() -> bool {
    let r = rt().lock().unwrap();
    r.file.is_some() || r.broken.is_some()
}

/// Is the start-authorization gate closed? True until an unlock or a
/// completed first-launch setup puts the VMK in memory (or the debug bypass
/// skips the gate) - the workspace never boots before it (CD-42 Task A).
pub fn gate_closed() -> bool {
    let r = rt().lock().unwrap();
    r.vmk.is_none() && !r.bypassed
}

/// Is the gate closed over an EXISTING vault (the unlock case, as opposed to
/// first-launch setup)?
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
/// `unlock_pass` (locked only), `setup_pass` (no vault - the mandatory
/// first-launch gate), `change_pass` (unlocked only).
pub fn begin_capture(purpose: &str) -> std::result::Result<(), String> {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return Err("busy".into());
    }
    let kind = match purpose {
        "unlock_pass" => CaptureKind::UnlockPass,
        "setup_pass" => CaptureKind::SetupPass,
        "change_pass" => CaptureKind::ChangePass,
        other => return Err(format!("unknown capture purpose: {other}")),
    };
    match kind {
        CaptureKind::UnlockPass => {
            if r.vmk.is_some() || (r.file.is_none() && r.broken.is_none()) {
                return Err("not locked".into());
            }
        }
        CaptureKind::SetupPass => {
            if r.file.is_some() || r.broken.is_some() {
                return Err("a vault already exists".into());
            }
        }
        CaptureKind::ChangePass => {
            if r.vmk.is_none() || r.file.is_none() {
                return Err("vault is not unlocked".into());
            }
        }
        CaptureKind::SetupConfirm | CaptureKind::ChangeConfirm | CaptureKind::RetuneKdf => {
            unreachable!()
        }
    }
    let input = SecretInput::new().map_err(|e| e.to_string())?;
    r.capture = Some(kind);
    r.input = Some(input);
    r.pending_pass = None;
    r.weak_pending = false;
    r.error = None;
    refresh_strength(&mut r);
    Ok(())
}

/// Cancel the current capture. Behind the closed gate this resets to a fresh
/// prompt for the gate's own flow (unlock, or first-launch setup - the setup
/// is mandatory, Esc must not orphan it); elsewhere it ends the flow.
pub fn cancel_capture() {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return;
    }
    r.input = None;
    r.pending_pass = None;
    r.pending_kdf = None;
    r.weak_pending = false;
    r.error = None;
    if r.vmk.is_none() && !r.bypassed {
        let vault_exists = r.file.is_some() || r.broken.is_some();
        r.capture = Some(if vault_exists {
            CaptureKind::UnlockPass
        } else {
            CaptureKind::SetupPass
        });
        r.input = SecretInput::new().ok();
    } else {
        r.capture = None;
    }
    refresh_strength(&mut r);
}

/// Is the host currently swallowing keystrokes into a secret buffer?
pub fn capture_active() -> bool {
    let r = rt().lock().unwrap();
    r.capture.is_some() && !r.busy
}

/// Route typed text into the capture buffer (also the paste path). Editing
/// re-evaluates the live strength meter and clears a parked weak override -
/// the warning always describes the CURRENT entry.
pub fn key_text(text: &str) {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return;
    }
    if let Some(input) = r.input.as_mut() {
        input.push_str(text);
    }
    r.weak_pending = false;
    refresh_strength(&mut r);
}

pub fn key_backspace() {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return;
    }
    if let Some(input) = r.input.as_mut() {
        input.backspace();
    }
    r.weak_pending = false;
    refresh_strength(&mut r);
}

/// Escape, never destructive-by-surprise (CD-44 A1): with text in the entry
/// it clears THE ENTRY only (plus any parked weak warning); with an empty
/// entry it steps BACK one step (confirm returns to the first entry, the
/// optional unlocked-session flows end). The mandatory flows (first-launch
/// setup, unlock) have no further back to go, so an empty-entry Escape only
/// clears a shown error there.
pub fn key_escape() {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return;
    }
    let has_text = r.input.as_ref().map(|i| i.len > 0).unwrap_or(false);
    if has_text {
        if let Some(input) = r.input.as_mut() {
            input.clear();
        }
        r.weak_pending = false;
        r.error = None;
        refresh_strength(&mut r);
        return;
    }
    match r.capture {
        // One step back: drop the banked first entry, re-type it.
        Some(CaptureKind::SetupConfirm) => {
            r.pending_pass = None;
            r.capture = Some(CaptureKind::SetupPass);
            r.input = SecretInput::new().ok();
            r.error = None;
            refresh_strength(&mut r);
        }
        Some(CaptureKind::ChangeConfirm) => {
            r.pending_pass = None;
            r.capture = Some(CaptureKind::ChangePass);
            r.input = SecretInput::new().ok();
            r.error = None;
            refresh_strength(&mut r);
        }
        // The optional unlocked-session flows end on Escape.
        Some(CaptureKind::ChangePass) | Some(CaptureKind::RetuneKdf) => {
            drop(r);
            cancel_capture();
        }
        // Mandatory flows: nowhere back to go; just clear a shown error.
        Some(CaptureKind::SetupPass) | Some(CaptureKind::UnlockPass) => {
            r.weak_pending = false;
            r.error = None;
        }
        None => {}
    }
}

/// Enter: advance the capture state machine. Cheap validations happen here;
/// anything with an Argon2 in it goes to a worker thread - the render loop
/// must never stall on a derivation.
pub fn key_submit() {
    let mut r = rt().lock().unwrap();
    if r.busy || r.capture.is_none() {
        return;
    }
    match r.capture.unwrap() {
        CaptureKind::UnlockPass => {
            if r.input.as_ref().map(|i| i.len).unwrap_or(0) == 0 {
                return; // empty Enter: nothing to do
            }
            // The password is the always-required factor; under 2FA the
            // Windows Hello assertion joins as the host-driven second step
            // (CD-43) - both factors together open the pair envelope.
            let pass = r.input.take();
            if r.file.as_ref().map(|f| f.required).unwrap_or(1) >= 2 {
                spawn_unlock_2fa(&mut r, pass);
            } else {
                spawn_unlock(&mut r, pass);
            }
        }
        CaptureKind::SetupPass => {
            let len = r.input.as_ref().map(|i| i.len).unwrap_or(0);
            if len < MIN_PASSPHRASE_LEN {
                r.error = Some(format!(
                    "the master password must be at least {MIN_PASSPHRASE_LEN} characters"
                ));
                return;
            }
            // A weak entry parks on the prominent warning (CD-42 Task B): the
            // ONLY way forward is the page's explicit accept_weak IPC -
            // repeated Enter never overrides. Editing re-evaluates.
            if r.strength.as_ref().map(|s| s.score).unwrap_or(0) < WEAK_SCORE_FLOOR {
                r.weak_pending = true;
                r.error = None;
                return;
            }
            r.pending_pass = r.input.take();
            r.input = SecretInput::new().ok();
            r.capture = Some(CaptureKind::SetupConfirm);
            r.error = None;
            refresh_strength(&mut r);
        }
        CaptureKind::SetupConfirm => {
            let confirm = r.input.take();
            let first = r.pending_pass.take();
            let (Some(first), Some(confirm)) = (first, confirm) else { return };
            if first.as_slice() != confirm.as_slice() {
                r.error = Some("the two entries do not match - start again".into());
                r.capture = Some(CaptureKind::SetupPass);
                r.input = SecretInput::new().ok();
                refresh_strength(&mut r);
                return;
            }
            drop(confirm);
            spawn_setup(&mut r, first);
        }
        CaptureKind::ChangePass => {
            let len = r.input.as_ref().map(|i| i.len).unwrap_or(0);
            if len < MIN_PASSPHRASE_LEN {
                r.error = Some(format!(
                    "the master password must be at least {MIN_PASSPHRASE_LEN} characters"
                ));
                return;
            }
            // Same informed-override gate as setup - a change IS setting the
            // master password.
            if r.strength.as_ref().map(|s| s.score).unwrap_or(0) < WEAK_SCORE_FLOOR {
                r.weak_pending = true;
                r.error = None;
                return;
            }
            r.pending_pass = r.input.take();
            r.input = SecretInput::new().ok();
            r.capture = Some(CaptureKind::ChangeConfirm);
            r.error = None;
            refresh_strength(&mut r);
        }
        CaptureKind::ChangeConfirm => {
            let confirm = r.input.take();
            let first = r.pending_pass.take();
            let (Some(first), Some(confirm)) = (first, confirm) else { return };
            if first.as_slice() != confirm.as_slice() {
                r.error = Some("the two entries do not match - start again".into());
                r.capture = Some(CaptureKind::ChangePass);
                r.input = SecretInput::new().ok();
                refresh_strength(&mut r);
                return;
            }
            drop(confirm);
            // Keep the passphrase method's CURRENT cost params on a plain
            // passphrase change; the re-tune flow stages different ones.
            let kdf = r
                .file
                .as_ref()
                .and_then(|f| f.methods.iter().find(|m| m.kind == MethodKind::Passphrase))
                .and_then(|m| m.kdf)
                .unwrap_or(KdfParams::PRODUCT);
            spawn_rewrap(&mut r, RewrapJob::ChangePass { pass: first, kdf });
        }
        CaptureKind::RetuneKdf => {
            let Some(input) = r.input.take() else { return };
            let Some(kdf) = r.pending_kdf.take() else {
                r.error = Some("no staged cost parameters".into());
                r.capture = None;
                return;
            };
            spawn_rewrap(&mut r, RewrapJob::RetuneKdf { pass: input, kdf });
        }
    }
}

/// Is the first-run passkey offer on screen? While it is, the shell holds
/// the gate view up (the vault itself is already unlocked, CD-44 D1).
pub fn passkey_offer_open() -> bool {
    rt().lock().unwrap().offer_passkey
}

/// Dismiss the first-run passkey offer: either declined ("not now") or
/// finished (enrolled). The vault is unaffected either way; this only ends
/// the UI step, and the workspace boots next.
pub fn dismiss_passkey_offer() {
    rt().lock().unwrap().offer_passkey = false;
}

/// The informed override (CD-42 Task B): the user deliberately proceeds with
/// a weak master password after the prominent warning. Only valid while a
/// weak submit is parked - the host trusts its OWN staged state, never the
/// page's claim - and advances to the confirm re-type exactly like a strong
/// submit would have.
pub fn accept_weak() -> std::result::Result<(), String> {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return Err("busy".into());
    }
    if !r.weak_pending {
        return Err("no weak entry is awaiting confirmation".into());
    }
    let next = match r.capture {
        Some(CaptureKind::SetupPass) => CaptureKind::SetupConfirm,
        Some(CaptureKind::ChangePass) => CaptureKind::ChangeConfirm,
        _ => return Err("no weak entry is awaiting confirmation".into()),
    };
    r.weak_pending = false;
    r.pending_pass = r.input.take();
    r.input = SecretInput::new().ok();
    r.capture = Some(next);
    r.error = None;
    refresh_strength(&mut r);
    Ok(())
}

/// A background re-wrap job from an unlocked session (CD-40 1c). Every job
/// re-wraps the VMK - the vault data is never re-encrypted.
enum RewrapJob {
    /// Replace the master password (fresh salt, `kdf` params).
    ChangePass { pass: SecretInput, kdf: KdfParams },
    /// Re-derive the password envelope under new cost params. The captured
    /// entry must VERIFY against the current envelope first - this flow tunes
    /// the cost, it must never silently change the password.
    RetuneKdf { pass: SecretInput, kdf: KdfParams },
}

/// Run a re-wrap on a worker thread with a cloned VMK; commit the new file on
/// success. Argon2 never runs under the runtime lock.
fn spawn_rewrap(r: &mut Runtime, job: RewrapJob) {
    let (Some(file), Some(vmk)) = (r.file.clone(), r.vmk.as_ref().and_then(|v| v.try_clone().ok()))
    else {
        r.error = Some("vault is not unlocked".into());
        return;
    };
    r.busy = true;
    r.error = None;
    let dir = r.dir.clone();
    std::thread::spawn(move || {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let result: Result<VaultFile> = (|| match job {
            RewrapJob::ChangePass { pass, kdf } => {
                change_passphrase(&file, &vmk, pass.as_slice(), &kdf, now_ms)
            }
            RewrapJob::RetuneKdf { pass, kdf } => {
                // Verify the entry IS the current password before re-deriving.
                let check = unlock(&file, &[Factor::Passphrase(pass.as_slice())])?;
                drop(check);
                change_passphrase(&file, &vmk, pass.as_slice(), &kdf, now_ms)
            }
        })()
        .and_then(|new| {
            new.save_to(&dir.join("vault.json"))?;
            Ok(new)
        });
        let mut r = rt().lock().unwrap();
        r.busy = false;
        r.capture = None;
        r.input = None;
        r.pending_pass = None;
        r.strength = None;
        r.weak_pending = false;
        match result {
            Ok(new) => {
                r.file = Some(new);
                r.error = None;
                r.outcome = Some(Outcome::Rewrapped);
                tracing::info!("vault re-wrapped");
            }
            Err(e) => {
                // The retune verify failure is the one caller-actionable case;
                // everything else keeps the uniform error discipline.
                r.error = Some(match e {
                    VaultError::Crypto => "the master password does not match".to_string(),
                    other => other.to_string(),
                });
                r.outcome = Some(Outcome::RewrapFailed);
            }
        }
    });
}

/// Bounds for user-tunable Argon2id cost (CD-40 1c): memory 16 MiB..=1 GiB,
/// passes 1..=10, lanes 1..=8. Anything below the RFC 9106 product default is
/// a WEAKENING and needs the confirm flag (the D-0040 gate discipline).
pub fn retune_kdf(m_cost_kib: u32, t_cost: u32, p_cost: u32, confirm: bool) -> std::result::Result<(), String> {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return Err("busy".into());
    }
    if r.vmk.is_none() || r.file.is_none() {
        return Err("vault is not unlocked".into());
    }
    if !(16 * 1024..=1024 * 1024).contains(&m_cost_kib) {
        return Err("memory cost must be between 16384 and 1048576 KiB".into());
    }
    if !(1..=10).contains(&t_cost) {
        return Err("passes must be between 1 and 10".into());
    }
    if !(1..=8).contains(&p_cost) {
        return Err("lanes must be between 1 and 8".into());
    }
    // Weakening = dropping below the RFC 9106 product default (the vetted
    // floor) OR below the user's own current cost - either way the offline
    // brute-force surface of vault.json gets cheaper, so the D-0040 gate
    // applies. Strengthening is always free.
    let current = r
        .file
        .as_ref()
        .and_then(|f| f.methods.iter().find(|m| m.kind == MethodKind::Passphrase))
        .and_then(|m| m.kdf)
        .unwrap_or(KdfParams::PRODUCT);
    let weakening = m_cost_kib < KdfParams::PRODUCT.m_cost_kib
        || t_cost < KdfParams::PRODUCT.t_cost
        || m_cost_kib < current.m_cost_kib
        || t_cost < current.t_cost;
    if weakening && !confirm {
        return Err(
            "lowering the passphrase cost below the default or the current setting requires confirmation"
                .into(),
        );
    }
    r.pending_kdf = Some(KdfParams { m_cost_kib, t_cost, p_cost });
    r.capture = Some(CaptureKind::RetuneKdf);
    r.input = SecretInput::new().ok();
    r.pending_pass = None;
    r.weak_pending = false;
    r.error = None;
    refresh_strength(&mut r);
    Ok(())
}

/// Change the unlock policy from the unlocked session: 1 = password-only,
/// 2 = password + passkey (2FA; needs the passkey enrolled - the core
/// refuses otherwise). Structural (a full envelope re-mint); BOTH directions
/// are confirm-gated and host-revalidated (D-0040 discipline): dropping 2FA
/// is a weakening, and ENABLING it is an informed-consent step - from then
/// on a lost Hello credential means an unrecoverable vault (no recovery
/// key, by design - the D-0062 stance, extended by D-0063). Cheap (AEAD
/// only, no KDF), so it runs synchronously.
pub fn set_policy(required: u8, confirm: bool) -> std::result::Result<(), String> {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return Err("busy".into());
    }
    let (Some(file), Some(vmk)) = (r.file.as_ref(), r.vmk.as_ref()) else {
        return Err("vault is not unlocked".into());
    };
    if required < file.required && !confirm {
        return Err("lowering the unlock policy requires confirmation".into());
    }
    if required >= 2 && file.required < 2 && !confirm {
        return Err(
            "enabling two-factor unlock requires confirmation - if the passkey is ever \
             lost, the vault cannot be opened (there is no recovery key, by design)"
                .into(),
        );
    }
    let new = set_required(file, vmk, required).map_err(|e| e.to_string())?;
    new.save_to(&r.dir.join("vault.json")).map_err(|e| e.to_string())?;
    r.file = Some(new);
    r.error = None;
    Ok(())
}

/// Remove the enrolled passkey (the only removable method - the core refuses
/// removing the password, and refuses removing the passkey while the 2FA
/// policy requires it). Synchronous like `set_policy` (AEAD only). The
/// OS-side Hello credential is deleted best-effort AFTER the vault state is
/// committed - the vault never depends on the OS credential store.
pub fn remove_enrolled_method(id: &str) -> std::result::Result<(), String> {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return Err("busy".into());
    }
    let (Some(file), Some(vmk)) = (r.file.as_ref(), r.vmk.as_ref()) else {
        return Err("vault is not unlocked".into());
    };
    #[cfg(all(not(test), windows))]
    let removed_cred = file.methods.iter().find(|m| m.id == id).and_then(|m| m.cred_id.clone());
    let new = remove_method(file, vmk, id).map_err(|e| e.to_string())?;
    new.save_to(&r.dir.join("vault.json")).map_err(|e| e.to_string())?;
    r.file = Some(new);
    r.error = None;
    #[cfg(all(not(test), windows))]
    if let Some(hex) = removed_cred
        && let Ok(cred) = hex_decode(&hex)
    {
        // Detached best-effort cleanup: this fn runs on the CEF UI thread
        // (the IPC handler) and still holds the runtime lock - the broker
        // call must block neither (CD-38 law). The vault result committed
        // above never depends on it.
        std::thread::spawn(move || crate::webauthn::delete_platform_credential(&cred));
    }
    Ok(())
}

/// Begin Windows Hello passkey enrollment from the unlocked session (CD-43
/// Task A). Host-validated: unlocked, no passkey yet (one max), platform
/// available. The modal MakeCredential + first PRF eval run on a worker
/// (two Hello prompts, by CTAP design - the PRF output only exists at
/// assertion time); success re-wraps the vault file with the new method.
/// At password-only the envelope set is unchanged - enrolling is what makes
/// the 2FA policy switch AVAILABLE, it never changes the policy itself.
pub fn begin_hello_enroll() -> std::result::Result<(), String> {
    let mut r = rt().lock().unwrap();
    if r.busy {
        return Err("busy".into());
    }
    let (Some(file), Some(vmk)) = (r.file.clone(), r.vmk.as_ref().and_then(|v| v.try_clone().ok()))
    else {
        return Err("vault is not unlocked".into());
    };
    if file.methods.iter().any(|m| m.kind == MethodKind::Passkey) {
        return Err("a passkey is already enrolled - remove it first".into());
    }
    let (available, api, hello_ready) = platform_info();
    if !available {
        return Err(format!(
            "Windows WebAuthn is unavailable on this system (API v{api})"
        ));
    }
    // The CD-44 A3 finding: with no Hello PIN/biometric enrolled, the
    // platform MakeCredential fails as a bare NotSupportedError. Refuse
    // up front with the actual next step instead.
    #[cfg(all(not(test), windows))]
    if !hello_ready {
        return Err(crate::webauthn::HELLO_SETUP_HINT.into());
    }
    #[cfg(any(test, not(windows)))]
    if !hello_ready {
        return Err("the platform authenticator is not available".into());
    }
    r.busy = true;
    r.hello = Some("enroll");
    r.error = None;
    let dir = r.dir.clone();
    drop(r);
    let hwnd = shell_hwnd();
    std::thread::spawn(move || {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let result: std::result::Result<VaultFile, String> = (|| {
            // The PRF eval value: per-passkey random, persisted non-secret on
            // the method (the OS applies the WebAuthn-spec hashing, D-0063).
            let salt: [u8; KEY_LEN] = rand_array().map_err(|e| e.to_string())?;
            let (cred_id, secret) = platform_enroll(hwnd, &salt)?;
            let vault_side: Result<VaultFile> = (|| {
                let new = enroll_passkey(
                    &file,
                    &vmk,
                    "Windows Hello",
                    secret.as_slice(),
                    Some(hex_encode(&cred_id)),
                    Some(hex_encode(&salt)),
                    now_ms,
                )?;
                new.save_to(&dir.join("vault.json"))?;
                Ok(new)
            })();
            match vault_side {
                Ok(new) => Ok(new),
                Err(e) => {
                    // The OS credential was minted but the vault could not
                    // commit it - delete the orphan best-effort, so a failed
                    // enrollment leaves no half-enrolled state on either side.
                    #[cfg(all(not(test), windows))]
                    crate::webauthn::delete_platform_credential(&cred_id);
                    Err(e.to_string())
                }
            }
        })();
        let mut r = rt().lock().unwrap();
        r.busy = false;
        r.hello = None;
        match result {
            Ok(new) => {
                r.file = Some(new);
                r.error = None;
                // Enrolling from the first-run offer answers it (CD-44 D1).
                r.offer_passkey = false;
                r.outcome = Some(Outcome::Rewrapped);
                tracing::info!("hello passkey enrolled - 2FA is now available");
            }
            Err(e) => {
                r.error = Some(e);
                r.outcome = Some(Outcome::RewrapFailed);
            }
        }
    });
    Ok(())
}

/// Unlock on a worker thread: derive → try envelopes → open the sealed state
/// → commit. The runtime lock is NOT held during the derivation.
fn spawn_unlock(r: &mut Runtime, pass: Option<SecretInput>) {
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

/// 2FA unlock (CD-43 Task B): the captured master password PLUS the Windows
/// Hello assertion, combined to open the `{password, passkey}` pair
/// envelope. The Hello modal shows while `hello = "assert"`. A failed or
/// cancelled Hello step returns to the password prompt WITH the typed entry
/// preserved (it stays in locked memory throughout) - retrying costs one
/// Enter, not a re-type; and since no password was checked yet, the message
/// is honest without being an oracle. There is NO Hello-only unlock: without
/// the password no envelope can open, structurally.
fn spawn_unlock_2fa(r: &mut Runtime, pass: Option<SecretInput>) {
    let Some(file) = r.file.clone() else {
        r.error = Some(r.broken.clone().unwrap_or_else(|| "vault unavailable".into()));
        r.outcome = Some(Outcome::UnlockFailed);
        r.input = SecretInput::new().ok();
        return;
    };
    let Some(pk) = file.methods.iter().find(|m| m.kind == MethodKind::Passkey).cloned() else {
        // A 2FA flag with no passkey cannot load (assert_model refuses it);
        // fail closed anyway rather than trusting that invariant here.
        r.error = Some("two-factor vault has no passkey method".into());
        r.outcome = Some(Outcome::UnlockFailed);
        r.input = SecretInput::new().ok();
        return;
    };
    r.busy = true;
    r.hello = Some("assert");
    r.error = None;
    let dir = r.dir.clone();
    let hwnd = shell_hwnd();
    std::thread::spawn(move || {
        let asserted: std::result::Result<SecretBuf, String> = (|| {
            let cred = hex_decode(pk.cred_id.as_deref().unwrap_or(""))
                .map_err(|_| "the passkey has no usable credential id".to_string())?;
            if cred.is_empty() {
                return Err("the passkey has no usable credential id".into());
            }
            let salt: [u8; KEY_LEN] = hex_decode(pk.salt.as_deref().unwrap_or(""))
                .ok()
                .and_then(|v| v.try_into().ok())
                .ok_or_else(|| "the passkey has no usable PRF salt".to_string())?;
            platform_assert(hwnd, &cred, &salt)
        })();
        match asserted {
            Ok(secret) => {
                let mut factors: Vec<Factor<'_>> = Vec::new();
                if let Some(p) = pass.as_ref() {
                    factors.push(Factor::Passphrase(p.as_slice()));
                }
                factors.push(Factor::MethodSecret { id: &pk.id, secret: secret.as_slice() });
                let result = unlock(&file, &factors);
                drop(factors);
                let sealed = result.as_ref().ok().map(|vmk| load_sealed(&dir, vmk));
                let mut r = rt().lock().unwrap();
                r.busy = false;
                r.hello = None;
                match result {
                    Ok(vmk) => {
                        r.vmk = Some(vmk);
                        r.sealed = sealed;
                        r.capture = None;
                        r.input = None;
                        r.pending_pass = None;
                        r.error = None;
                        r.outcome = Some(Outcome::Unlocked);
                        tracing::info!("vault unlocked (password + passkey)");
                    }
                    Err(_) => {
                        // Uniform by design (VaultError::Crypto carries no
                        // oracle) - the Hello step succeeded, so this is a
                        // factor mismatch, indistinguishable which.
                        r.error = Some("unlock failed".into());
                        r.capture = Some(CaptureKind::UnlockPass);
                        r.input = SecretInput::new().ok();
                        r.outcome = Some(Outcome::UnlockFailed);
                    }
                }
            }
            Err(e) => {
                // The Hello step itself failed - before any password check.
                let mut r = rt().lock().unwrap();
                r.busy = false;
                r.hello = None;
                r.error = Some(e);
                r.capture = Some(CaptureKind::UnlockPass);
                r.input = pass; // the typed password survives for a retry
                r.outcome = Some(Outcome::UnlockFailed);
            }
        }
    });
}

/// Set up the vault on a worker thread: create → save `vault.json` → migrate
/// the plaintext identity seed into the sealed state → commit an UNLOCKED
/// session.
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
                r.weak_pending = false;
                refresh_strength(&mut r);
                r.outcome = Some(Outcome::SetupFailed);
                return;
            }
        };
        // Migrate what is sensitive TODAY into the sealed state: the persisted
        // identity seed (it keys the fingerprint farbling - linkage material),
        // then remove the plaintext rows. Session/layout metadata stays in
        // state.db per the ticket (do not seal what doesn't need sealing).
        // Not under test: the unit suite must never open (or delete rows from)
        // the developer's real state.db - the runtime test drives the sealed
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
        let NewVault { file, vmk } = committed;
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
        r.strength = None;
        r.weak_pending = false;
        // Offer the passkey right after the master password (CD-44 D1) -
        // optional, declinable, and only where it can actually be taken up.
        r.offer_passkey = platform_info().2;
        r.outcome = Some(Outcome::SetupDone);
        tracing::info!("vault created - session unlocked");
    });
}

/// Queue "lock now": the shell relaunches the process cold (D-0059) - every
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
/// abnormal termination is covered by the OS zeroing freed pages - the CD-33
/// tier model).
pub fn wipe_for_exit() {
    let mut r = rt().lock().unwrap();
    r.vmk = None;
    r.input = None;
    r.pending_pass = None;
    r.sealed = None;
}

/// The vault state snapshot the lock/settings pages render (pushed on change
/// via `browser::set_vault_state`, pulled on load via `get_vault_state`).
/// Carries counts and states - NEVER a secret (the iron law: the typed
/// password stays in host-locked memory; the page renders dots from `chars`).
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
    // The config surface (CD-40 1c): enrolled methods, policy, KDF cost -
    // honest metadata, no secrets.
    let methods: Vec<serde_json::Value> = r
        .file
        .as_ref()
        .map(|f| {
            f.methods
                .iter()
                .map(|m| {
                    serde_json::json!({
                        "id": m.id,
                        "kind": match m.kind {
                            MethodKind::Passphrase => "passphrase",
                            MethodKind::Passkey => "passkey",
                        },
                        "label": m.label,
                        "created_ms": m.created_ms,
                        "removable": m.kind.hardware(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let kdf = r
        .file
        .as_ref()
        .and_then(|f| f.methods.iter().find(|m| m.kind == MethodKind::Passphrase))
        .and_then(|m| m.kdf)
        .map(|k| serde_json::json!({ "m_cost_kib": k.m_cost_kib, "t_cost": k.t_cost, "p_cost": k.p_cost }));
    // The live meter (CD-42 Task B): present only while a NEW master password
    // is being typed. Score, criteria and zxcvbn's canned feedback strings -
    // the password characters themselves never cross (the iron law).
    let strength = r.strength.as_ref().map(|s| {
        serde_json::json!({
            "score": s.score,
            "len_ok": s.chars >= TARGET_LEN,
            "target_len": TARGET_LEN,
            "warning": s.warning,
            "suggestions": s.suggestions,
        })
    });
    // Honest platform capability for the config surface (CD-43/CD-44):
    // whether the OS WebAuthn layer can serve the passkey path at all,
    // whether Windows Hello is actually set up (a live fact, re-probed), and
    // the Hello modal state while a worker holds one open.
    let (wa_available, wa_api, wa_hello) = platform_info();
    serde_json::json!({
        "vault": vault,
        "capture": r.capture.map(|c| c.as_str()),
        "chars": r.input.as_ref().map(|i| i.chars()).unwrap_or(0),
        "required": r.file.as_ref().map(|f| f.required).unwrap_or(1),
        "methods": methods,
        "kdf": kdf,
        "strength": strength,
        "weak_pending": r.weak_pending,
        "hello": r.hello,
        "offer_passkey": r.offer_passkey,
        "webauthn": { "available": wa_available, "api": wa_api, "hello_ready": wa_hello },
        "busy": r.busy,
        "error": r.error,
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

/// Read a sealed string tenant (unlocked sessions only - a locked or
/// bypassed vault yields None, never a plaintext fallback).
pub fn sealed_get(key: &str) -> Option<String> {
    let r = rt().lock().unwrap();
    let sealed = r.sealed.as_ref()?;
    r.vmk.as_ref()?;
    sealed.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// Write a sealed string tenant and persist the sealed state (unlocked
/// sessions only; a no-op otherwise - fail-closed, nothing falls back to
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
        strength: None,
        weak_pending: false,
        hello: None,
        offer_passkey: false,
        busy: false,
        error: None,
        outcome: None,
        relaunch: false,
        sealed: None,
        kdf,
        pending_kdf: None,
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

    /// Fast Argon2 params for tests only - the code path is identical (params
    /// are stored data), the cost is not what these tests assert.
    const TEST_KDF: KdfParams = KdfParams { m_cost_kib: 64, t_cost: 1, p_cost: 1 };
    const NOW: u64 = 1_753_000_000_000;

    fn fresh() -> NewVault {
        create(b"correct horse battery staple", &TEST_KDF, NOW).expect("create")
    }

    // --- SecretBuf ----------------------------------------------------------

    /// Locked allocation arrives zeroed, holds data, wipes on demand - and
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

    // --- Setup + unlock ------------------------------------------------------

    /// CD-42 acceptance 3 (headless half): setup creates a VMK wrapped by the
    /// master password alone - one method, one envelope, password-only policy.
    #[test]
    fn create_unlocks_with_master_password_only() {
        let nv = fresh();
        assert_eq!(nv.file.methods.len(), 1, "the master password is the sole root");
        assert_eq!(nv.file.envelopes.len(), 1, "one envelope: {{password}}");
        assert_eq!(nv.file.required, 1);

        let vmk = unlock(&nv.file, &[Factor::Passphrase(b"correct horse battery staple")])
            .expect("the master password unlocks");
        assert_eq!(vmk.as_slice(), nv.vmk.as_slice());
    }

    /// Wrong factors and tampered blobs all fail with the uniform crypto
    /// error - no oracle distinguishes them.
    #[test]
    fn wrong_password_and_tampered_envelope_fail_closed() {
        let nv = fresh();
        assert_eq!(
            unlock(&nv.file, &[Factor::Passphrase(b"wrong horse battery staple")]).unwrap_err(),
            VaultError::Crypto
        );

        // Flip one ciphertext byte in the envelope → nothing unlocks.
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
    }

    /// CD-42 Task D (headless half): the 2FA policy is enforced STRUCTURALLY -
    /// the only envelope is {password, passkey}, single factors find no
    /// candidate, and hand-editing the `required` field back to 1 changes
    /// nothing because no password-only envelope exists.
    #[test]
    fn two_factor_policy_is_structural_not_a_flag() {
        let nv = fresh();
        let prf = SecretBuf::random(KEY_LEN).unwrap(); // stands in for the PRF output
        let with_pk = enroll_passkey(&nv.file, &nv.vmk, "YubiKey 5", prf.as_slice(), None, None, NOW)
            .expect("enroll from unlocked session");
        let pk_id = with_pk
            .methods
            .iter()
            .find(|m| m.kind == MethodKind::Passkey)
            .unwrap()
            .id
            .clone();

        let two = set_required(&with_pk, &nv.vmk, 2).expect("policy re-wrap");
        assert_eq!(two.required, 2);
        assert_eq!(two.envelopes.len(), 1, "one pair envelope for {{pw, passkey}}");

        assert!(unlock(&two, &[Factor::Passphrase(b"correct horse battery staple")]).is_err());
        assert!(
            unlock(&two, &[Factor::MethodSecret { id: &pk_id, secret: prf.as_slice() }]).is_err(),
            "the passkey alone opens nothing - the password is always required"
        );
        let vmk = unlock(
            &two,
            &[
                Factor::Passphrase(b"correct horse battery staple"),
                Factor::MethodSecret { id: &pk_id, secret: prf.as_slice() },
            ],
        )
        .expect("password + passkey together unlock");
        assert_eq!(vmk.as_slice(), nv.vmk.as_slice());

        // The attacker edit: required 2 → 1. Still only the pair envelope.
        let mut edited = two.clone();
        edited.required = 1;
        assert!(
            unlock(&edited, &[Factor::Passphrase(b"correct horse battery staple")]).is_err(),
            "downgrading the flag must not weaken the cryptography"
        );

        // The AAD transplant: relabel the pair envelope as password-only.
        // Untouched ciphertext + the right password still fail - the AAD
        // binds the exact method set.
        let mut renamed = two.clone();
        renamed.required = 1;
        renamed.envelopes[0].method_ids = vec!["passphrase".to_string()];
        assert!(
            unlock(&renamed, &[Factor::Passphrase(b"correct horse battery staple")]).is_err()
        );

        // And back down to password-only through the proper path.
        let one = set_required(&two, &nv.vmk, 1).expect("back to password-only");
        assert!(unlock(&one, &[Factor::Passphrase(b"correct horse battery staple")]).is_ok());
        assert!(
            unlock(&one, &[Factor::MethodSecret { id: &pk_id, secret: prf.as_slice() }]).is_err(),
            "at password-only the passkey STILL opens nothing (escrow, not envelope)"
        );
    }

    /// CD-42 Task C (core half): the passkey enrolls from an unlocked session
    /// as the ONLY additional factor - it gains an escrow, never its own
    /// envelope; a second passkey is refused; removal follows the policy.
    #[test]
    fn passkey_is_additional_never_a_replacement() {
        let nv = fresh();
        let prf = SecretBuf::random(KEY_LEN).unwrap();
        let with_pk = enroll_passkey(&nv.file, &nv.vmk, "YubiKey 5", prf.as_slice(), None, None, NOW)
            .expect("enroll from unlocked session");
        assert_eq!(with_pk.methods.len(), 2);
        assert_eq!(
            with_pk.envelopes.len(),
            1,
            "password-only keeps the single {{password}} envelope"
        );
        assert_eq!(with_pk.escrows.len(), 2, "the passkey is escrowed for a 2FA switch");

        let pk_id = with_pk
            .methods
            .iter()
            .find(|m| m.kind == MethodKind::Passkey)
            .unwrap()
            .id
            .clone();
        assert!(
            unlock(&with_pk, &[Factor::MethodSecret { id: &pk_id, secret: prf.as_slice() }])
                .is_err(),
            "the passkey never replaces the password"
        );
        assert!(
            unlock(&with_pk, &[Factor::Passphrase(b"correct horse battery staple")]).is_ok()
        );

        // At most one passkey (D-0062).
        let prf2 = SecretBuf::random(KEY_LEN).unwrap();
        assert!(matches!(
            enroll_passkey(&with_pk, &nv.vmk, "Second key", prf2.as_slice(), None, None, NOW),
            Err(VaultError::Policy(_))
        ));

        // Removal at password-only works; while 2FA requires it, it refuses.
        let two = set_required(&with_pk, &nv.vmk, 2).unwrap();
        assert!(matches!(
            remove_method(&two, &nv.vmk, &pk_id),
            Err(VaultError::Policy(_))
        ));
        let one = set_required(&two, &nv.vmk, 1).unwrap();
        let removed = remove_method(&one, &nv.vmk, &pk_id).expect("passkey is removable at 1FA");
        assert_eq!(removed.methods.len(), 1);
    }

    /// CD-42 Task D/E: the structural invariants hold - the master password
    /// cannot be removed, the policy accepts only 1 and 2, 2FA needs the
    /// passkey, and pathological files are refused.
    #[test]
    fn restricted_model_invariants_hold() {
        let nv = fresh();
        assert!(matches!(
            remove_method(&nv.file, &nv.vmk, "passphrase"),
            Err(VaultError::Policy(_))
        ));
        assert!(matches!(set_required(&nv.file, &nv.vmk, 0), Err(VaultError::Policy(_))));
        assert!(matches!(set_required(&nv.file, &nv.vmk, 3), Err(VaultError::Policy(_))));
        assert!(
            matches!(set_required(&nv.file, &nv.vmk, 2), Err(VaultError::Policy(_))),
            "2FA without an enrolled passkey is refused"
        );

        // A hand-built pathological file: a passkey-only envelope (a password
        // bypass). The invariant refuses it (load would too).
        let mut bad = nv.file.clone();
        bad.methods.push(Method {
            id: "passkey-dead".into(),
            kind: MethodKind::Passkey,
            label: "K".into(),
            created_ms: NOW,
            kdf: None,
            salt: None,
            cred_id: None,
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
        assert!(matches!(assert_model(&bad), Err(VaultError::Policy(_))));

        // Two password methods → refused.
        let mut twin = nv.file.clone();
        twin.methods.push(Method {
            id: "passphrase-2".into(),
            kind: MethodKind::Passphrase,
            label: "P2".into(),
            created_ms: NOW,
            kdf: None,
            salt: None,
            cred_id: None,
        });
        assert!(matches!(assert_model(&twin), Err(VaultError::Policy(_))));

        // A 2FA flag over a password-only envelope set → refused (the shape
        // must match the claimed policy in BOTH directions).
        let mut mismatched = nv.file.clone();
        mismatched.required = 2;
        assert!(matches!(assert_model(&mismatched), Err(VaultError::Policy(_))));
    }

    /// Rotation re-wraps atomically: the old password dies with the old file,
    /// the new one works, and the VMK (and thus the vault data) is unchanged.
    #[test]
    fn change_master_password_rotates_cleanly() {
        let nv = fresh();
        let changed =
            change_passphrase(&nv.file, &nv.vmk, b"a brand new passphrase", &TEST_KDF, NOW + 1)
                .expect("change master password");
        assert!(unlock(&changed, &[Factor::Passphrase(b"correct horse battery staple")]).is_err());
        let vmk = unlock(&changed, &[Factor::Passphrase(b"a brand new passphrase")]).unwrap();
        assert_eq!(vmk.as_slice(), nv.vmk.as_slice(), "the VMK never changes on re-wrap");

        // Too-short replacement is refused before any crypto runs.
        assert!(matches!(
            change_passphrase(&nv.file, &nv.vmk, b"short", &TEST_KDF, NOW),
            Err(VaultError::Policy(_))
        ));
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

        // A v1 file (the retired CD-40 recovery-key model) → refused with the
        // reset message, not a serde parse error (D-0062).
        std::fs::write(
            &path,
            r#"{"version":1,"required":1,"methods":[{"id":"recovery","kind":"recovery_key","label":"Recovery key","created_ms":0}],"envelopes":[],"escrows":[]}"#,
        )
        .unwrap();
        match VaultFile::load_from(&path) {
            Err(VaultError::Format(msg)) => {
                assert!(msg.contains("retired recovery-key model"), "got: {msg}");
            }
            other => panic!("v1 file must be refused with a reset message, got {other:?}"),
        }

        // A file whose shape violates the model (no password method) →
        // refused at load.
        let mut brick = loaded.clone();
        brick.methods.retain(|m| m.kind != MethodKind::Passphrase);
        std::fs::write(&path, serde_json::to_string(&brick).unwrap()).unwrap();
        assert!(matches!(VaultFile::load_from(&path), Err(VaultError::Policy(_))));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The vault file must never contain the master password or any wrapping
    /// key in the clear. Serializing and scanning is a cheap tripwire for the
    /// classic serde mistake (a secret field slipping into a Serialize
    /// derive).
    #[test]
    fn vault_json_contains_no_plaintext_secrets() {
        let nv = fresh();
        let json = serde_json::to_string(&nv.file).unwrap();
        assert!(!json.contains("correct horse battery staple"));
        assert!(!json.contains(&hex_encode(nv.vmk.as_slice())));
    }

    /// The product Argon2id parameters are the documented RFC 9106 second
    /// recommendation - a regression guard against silent weakening.
    #[test]
    fn product_kdf_params_match_rfc_9106_second_recommendation() {
        assert_eq!(KdfParams::PRODUCT.m_cost_kib, 65536, "64 MiB");
        assert_eq!(KdfParams::PRODUCT.t_cost, 3);
        assert_eq!(KdfParams::PRODUCT.p_cost, 4);
    }

    // --- Stage 1b: host capture + runtime -----------------------------------

    /// SecretInput edits correctly across multi-byte characters and wipes on
    /// clear - the host-captured entry buffer behind the iron law.
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

    // --- Strength meter (CD-42 Task B) --------------------------------------

    /// The meter is host-computed on the locked input: a dictionary word
    /// scores below the weak floor with a canned warning, a random-looking
    /// long entry scores 4, and the evaluation cap keeps very long input
    /// bounded while the reported char count stays honest.
    #[test]
    fn strength_meter_is_host_computed_and_bounded() {
        let mut i = SecretInput::new().unwrap();
        i.push_str("password");
        let weak = eval_strength(&i);
        assert!(weak.score < WEAK_SCORE_FLOOR, "'password' must be weak");
        assert!(weak.warning.is_some(), "a canned warning explains the weakness");

        i.clear();
        i.push_str("vB7#kePq9wRx2xLm");
        let strong = eval_strength(&i);
        assert_eq!(strong.score, 4, "a random 16-char mix is very strong");
        assert!(strong.warning.is_none());
        assert_eq!(strong.chars, 16);

        // 240 chars: evaluated over the 64-char prefix cap (bounded CPU on
        // the UI thread), char count reported in full.
        i.clear();
        for _ in 0..30 {
            i.push_str("abcdefgh");
        }
        let long = eval_strength(&i);
        assert_eq!(long.chars, 240);
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

    /// The full runtime flow, end to end against a temp dir: mandatory setup
    /// through the host-capture state machine (mismatch retry included), a
    /// sealed tenant surviving a simulated relaunch, and a failed then
    /// successful master-password unlock. ONE test drives the whole flow
    /// because the runtime is a process-wide singleton - two parallel tests
    /// would interleave.
    #[test]
    fn runtime_setup_lock_unlock_flow() {
        let dir = std::env::temp_dir().join(format!("cdvault-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        test_reset_runtime(&dir, TEST_KDF);

        assert!(!has_vault());
        assert!(!is_locked(), "no vault → nothing to unlock");
        assert!(gate_closed(), "no vault → the gate is CLOSED on mandatory setup (CD-42)");
        assert!(begin_capture("unlock_pass").is_err(), "nothing to unlock yet");

        // The master password used through the flow - random-looking, so the
        // meter scores it strong AND the iron-law tripwire below can assert
        // it never appears in the state JSON.
        const STRONG: &str = "vB7#kePq9wRx2xLm";

        // Setup. A weak entry parks on the prominent warning: Enter never
        // overrides, editing re-evaluates (CD-42 Task B).
        begin_capture("setup_pass").unwrap();
        assert!(state_json().contains("\"strength\""), "the live meter is on during setup");
        key_text("password");
        key_submit();
        assert!(state_json().contains("\"weak_pending\":true"), "weak submit parks");
        key_submit();
        assert!(
            state_json().contains("\"weak_pending\":true"),
            "repeated Enter does not override the warning"
        );
        key_backspace();
        assert!(
            state_json().contains("\"weak_pending\":false"),
            "editing clears the parked warning and re-evaluates"
        );
        for _ in 0..8 {
            key_backspace();
        }

        // Strong entry: the iron-law tripwire - the typed characters never
        // appear in the state JSON - then a mismatching confirm (retry), then
        // a clean create.
        key_text(STRONG);
        assert!(
            !state_json().contains("vB7#kePq"),
            "IRON LAW: the typed password must never reach the renderer state"
        );
        key_submit(); // strong → straight to the confirm step
        assert!(state_json().contains("setup_confirm"));
        key_text("not the same at all");
        key_submit();
        assert!(
            state_json().contains("do not match"),
            "mismatching confirm restarts setup with an error"
        );

        // Esc semantics (CD-44 A1): with text it clears the ENTRY only; on
        // an empty confirm step it goes BACK one step; on the mandatory
        // first step it never aborts the flow.
        key_text(STRONG);
        key_submit(); // → confirm step again
        key_text("abc");
        key_escape();
        assert!(state_json().contains("\"chars\":0"), "Esc clears the entry");
        assert!(
            state_json().contains("setup_confirm"),
            "Esc with text stays on the same step"
        );
        key_escape();
        assert!(
            state_json().contains("setup_pass"),
            "empty Esc steps back to the first entry"
        );
        key_escape();
        assert!(
            state_json().contains("setup_pass"),
            "the mandatory setup never Esc-aborts"
        );

        key_text(STRONG);
        key_submit();
        key_text(STRONG);
        key_submit();
        assert_eq!(wait_outcome(), Outcome::SetupDone);
        assert!(is_unlocked(), "setup leaves the session unlocked");
        assert!(!gate_closed(), "setup opens the gate");
        assert!(dir.join("vault.json").exists());
        // The first-run passkey offer (CD-44 D1) is a UI step only: it is
        // raised solely where a passkey could actually be taken up, the
        // vault is already usable, and declining just ends the step.
        assert!(
            !passkey_offer_open(),
            "no offer without a usable platform authenticator"
        );

        // A sealed tenant written while unlocked…
        sealed_set("identity_seed", "00aa11bb");
        assert_eq!(sealed_get("identity_seed").as_deref(), Some("00aa11bb"));

        // …survives a relaunch (runtime reset), unreadable while locked.
        test_reset_runtime(&dir, TEST_KDF);
        assert!(is_locked(), "vault present + no VMK → gate closed");
        assert_eq!(sealed_get("identity_seed"), None, "sealed stays sealed while locked");

        // Wrong password fails (uniform error), the correct one opens the gate.
        begin_capture("unlock_pass").unwrap();
        assert!(
            state_json().contains("\"strength\":null"),
            "no meter on the unlock prompt - it exists only while SETTING a password"
        );
        key_text("wrong horse battery staple");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::UnlockFailed);
        assert!(is_locked());
        key_text(STRONG);
        key_submit();
        assert_eq!(wait_outcome(), Outcome::Unlocked);
        assert!(is_unlocked());
        assert_eq!(
            sealed_get("identity_seed").as_deref(),
            Some("00aa11bb"),
            "the sealed tenant is back after unlock"
        );

        // --- The config surface, driven end to end while unlocked -----------

        // Policy: 2FA needs an enrolled passkey - none exists, so the switch
        // refuses even WITH the consent flag (CD-42 Task D).
        assert!(set_policy(2, true).is_err(), "2FA without a passkey is refused");
        assert!(state_json().contains("\"required\":1"));

        // Change the master password to a WEAK one via the informed override
        // (CD-42 acceptance 1: a weak password CAN be set deliberately).
        begin_capture("change_pass").unwrap();
        assert!(accept_weak().is_err(), "nothing parked yet - the IPC cannot skip ahead");
        key_text("password");
        key_submit();
        assert!(state_json().contains("\"weak_pending\":true"));
        accept_weak().expect("the deliberate override proceeds");
        key_text("password");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::Rewrapped);
        assert!(is_unlocked(), "the session stays unlocked across a re-wrap");

        // Relaunch: the OLD password is dead; the deliberately-weak new one
        // really unlocks (the override produced a working vault).
        test_reset_runtime(&dir, TEST_KDF);
        begin_capture("unlock_pass").unwrap();
        key_text(STRONG);
        key_submit();
        assert_eq!(wait_outcome(), Outcome::UnlockFailed);
        key_text("password");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::Unlocked);

        // KDF re-tune: bounds + weakening gate enforced, the captured entry is
        // VERIFIED against the current envelope (a wrong entry must not
        // silently become the new password), then the cost really changes.
        // No meter here - the CURRENT password is being entered, not a new one.
        assert!(retune_kdf(1024, 1, 1, true).is_err(), "below the memory floor");
        assert!(
            retune_kdf(16 * 1024, 1, 1, false).is_err(),
            "weaker than the product default needs confirmation"
        );
        retune_kdf(16 * 1024, 1, 1, true).unwrap();
        assert!(state_json().contains("\"strength\":null"));
        key_text("not the passphrase at all");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::RewrapFailed);
        assert!(state_json().contains("does not match"));
        retune_kdf(16 * 1024, 1, 1, true).unwrap();
        key_text("password");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::Rewrapped);
        assert!(state_json().contains("\"m_cost_kib\":16384"));

        // --- CD-43: Hello passkey enroll + 2FA unlock (mock platform) -------

        // No mock "platform" installed → enrollment is host-refused, honestly.
        assert!(begin_hello_enroll().is_err(), "platform unavailable is refused");

        // Enroll through the platform seam: the worker mints the credential
        // id + PRF secret, re-wraps the file, and 2FA becomes available.
        const PRF: [u8; KEY_LEN] = [0x5a; KEY_LEN];
        *test_prf().lock().unwrap() = Some(PRF.to_vec());

        // With a platform authenticator present, a fresh setup DOES raise the
        // offer, and dismissing it is what lets the workspace boot (D1).
        {
            let mut r = rt().lock().unwrap();
            r.offer_passkey = platform_info().2;
        }
        assert!(passkey_offer_open(), "the offer is raised where Hello can serve it");
        dismiss_passkey_offer();
        assert!(!passkey_offer_open(), "declining ends the step, vault untouched");
        assert!(is_unlocked(), "declining never affects the vault itself");
        begin_hello_enroll().expect("enroll via the mock platform");
        assert_eq!(wait_outcome(), Outcome::Rewrapped);
        assert!(begin_hello_enroll().is_err(), "one passkey max");
        let state: serde_json::Value = serde_json::from_str(&state_json()).unwrap();
        let pk_id = state["methods"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["kind"] == "passkey")
            .expect("passkey enrolled")["id"]
            .as_str()
            .unwrap()
            .to_string();

        // The persisted method carries the assertion inputs - non-secret
        // credential id + PRF eval salt, hex (CD-43 plumbing check).
        {
            let r = rt().lock().unwrap();
            let m = r
                .file
                .as_ref()
                .unwrap()
                .methods
                .iter()
                .find(|m| m.kind == MethodKind::Passkey)
                .unwrap();
            assert_eq!(hex_decode(m.cred_id.as_deref().unwrap()).unwrap(), TEST_CRED_ID);
            assert_eq!(hex_decode(m.salt.as_deref().unwrap()).unwrap().len(), KEY_LEN);
        }

        // 2FA on: enabling is an informed-consent step (D-0063 - a lost
        // passkey then means an unrecoverable vault), so the bare switch is
        // host-refused; with the acknowledgment it applies. The passkey is
        // then locked in by the policy.
        assert!(
            set_policy(2, false).is_err(),
            "enabling 2FA without the consent flag is refused"
        );
        set_policy(2, true).expect("2FA with a passkey enrolled + consent");
        assert!(
            remove_enrolled_method(&pk_id).is_err(),
            "the 2FA policy refuses removing its second factor"
        );

        // Relock → 2FA unlock: password + the (mock) Hello assertion.
        test_reset_runtime(&dir, TEST_KDF);
        assert!(begin_hello_enroll().is_err(), "enrollment needs the unlocked session");
        begin_capture("unlock_pass").unwrap();
        key_text("password");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::Unlocked);
        assert!(is_unlocked(), "password + passkey assertion together open the pair envelope");

        // Wrong password + a VALID assertion still fails - uniform error.
        test_reset_runtime(&dir, TEST_KDF);
        begin_capture("unlock_pass").unwrap();
        key_text("not the password");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::UnlockFailed);
        assert!(state_json().contains("unlock failed"));

        // A failed Hello step reports honestly (no password was checked, so
        // no oracle) and PRESERVES the typed password for a one-Enter retry.
        *test_prf().lock().unwrap() = None;
        key_text("password");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::UnlockFailed);
        assert!(state_json().contains("mock platform unavailable"));
        assert!(
            state_json().contains("\"chars\":8"),
            "the typed password survives a failed second factor"
        );
        *test_prf().lock().unwrap() = Some(PRF.to_vec());
        key_submit(); // one Enter retries with the preserved entry
        assert_eq!(wait_outcome(), Outcome::Unlocked);

        // Dropping back: 2FA→password-only is a weakening (confirm-gated);
        // then the passkey removes cleanly - no brick (CD-43 Task C).
        assert!(set_policy(1, false).is_err(), "dropping 2FA needs confirmation");
        set_policy(1, true).unwrap();
        remove_enrolled_method(&pk_id).expect("passkey removable at password-only");
        test_reset_runtime(&dir, TEST_KDF);
        begin_capture("unlock_pass").unwrap();
        key_text("password");
        key_submit();
        assert_eq!(wait_outcome(), Outcome::Unlocked);
        assert!(is_unlocked(), "password-only unlock works after removal");

        // Lock request queues a relaunch; wipe drops every secret.
        request_lock();
        assert!(take_relaunch());
        assert!(!take_relaunch(), "one-shot");
        wipe_for_exit();
        assert!(!is_unlocked());

        std::fs::remove_dir_all(&dir).ok();
    }
}
