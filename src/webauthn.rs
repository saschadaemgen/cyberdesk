//! Windows Hello / WebAuthn PRF integration (CD-43, D-0063).
//!
//! The vault passkey's platform half: create a credential on the Windows
//! platform authenticator (Hello) with the CTAP `hmac-secret` capability, and
//! evaluate the PRF at assertion time to derive the stable 32-byte method
//! secret the envelope layer consumes (`vault::enroll_passkey` /
//! `Factor::MethodSecret`). This module contains NO cryptography of its own -
//! it marshals the OS `webauthn.dll` API and moves the returned secret into
//! locked memory ([`vault::SecretBuf`]).
//!
//! ## The Task-0 determination (verified at source, D-0063)
//!
//! * The assertion-time PRF path is **API v4-era**, complete in the pinned
//!   `windows-sys 0.61.2` (v7 header): `pHmacSecretSaltValues` in
//!   `GET_ASSERTION_OPTIONS` (struct v6) + the `pHmacSecret` output in
//!   `WEBAUTHN_ASSERTION` (struct v3). API **v8 added create-time eval only**
//!   (`pPRFGlobalEval`) - a convenience this module replaces with an eval
//!   assertion right after enrollment. No v8 FFI is needed; no crate bump.
//! * **Salt hashing is the DLL's job by default**: values passed via
//!   `pHmacSecretSaltValues` are converted per the WebAuthn PRF spec -
//!   `SHA-256(UTF8("WebAuthn PRF") || 0x00 || value)` - unless the caller
//!   opts into RAW salts via `WEBAUTHN_AUTHENTICATOR_HMAC_SECRET_VALUES_FLAG`
//!   (webauthn.h, the comment above WEBAUTHN_HMAC_SECRET_SALT). We do NOT set
//!   the flag: the OS applies the spec hashing to our stored eval value, so
//!   the derived secret matches what any spec-compliant PRF client would
//!   compute from the same input, and no hashing is hand-rolled here.
//! * Windows Hello gained the hmac-secret capability with the Feb-2026
//!   cumulative (KB5077181, build 26200.7840+); the DLL's API version number
//!   is a proxy, so the AUTHORITATIVE capability check is empirical - the
//!   enrollment's eval assertion either returns 32 bytes or enrollment fails
//!   with the OS error name (honest, fail-closed).
//!
//! ## Hard-won FFI rules encoded here
//!
//! * Output structs are read only up to the RETURNED `dwVersion` - Windows
//!   Hello on older builds returns downlevel structs, and reading past them
//!   is an access violation (kanidm/webauthn-rs issue #262; their fix is the
//!   same gate).
//! * Every input struct is stamped with its `*_CURRENT_VERSION` and fully
//!   zero-initialized first; every marshalled buffer outlives the call.
//! * API-allocated output buffers holding secret bytes are zeroized IN PLACE
//!   before `WebAuthNFreeAssertion` (bounded residual: copies inside
//!   webauthn.dll / the authenticator broker are out of reach - documented
//!   in cyberdesk-security.md).
//! * The calls are modal (they show the Hello UI over `hwnd`) and BLOCK -
//!   they must run on a vault worker thread, never the render loop or the
//!   CEF UI thread (CD-38 threading law).
//!
//! The attestation itself is deliberately NOT validated: the PRF output is
//! the payload, and the vault's security rests on the AEAD envelope (a wrong
//! or forged secret simply fails to unwrap the VMK - uniform error). The
//! RP id below is a stable local identifier, not a real origin; changing it
//! would orphan enrolled credentials.

#![cfg(windows)]
// The enroll/assert surface is consumed by the vault runtime as CD-43's
// stages land; keep the module warm like store.rs/vault.rs before it.
#![allow(dead_code)]

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Networking::WindowsWebServices::*;
use windows_sys::core::BOOL;

use zeroize::Zeroize;

use crate::vault::{KEY_LEN, SecretBuf};

/// The RP identity of the vault's credential - stable forever (it scopes the
/// enrolled credential; a change would orphan it). Shown by the Hello UI.
const RP_ID: &str = "cyberdesk.local";
const RP_NAME: &str = "CyberDesk Vault";
/// The origin recorded in client data. Nothing verifies it (the assertion is
/// consumed locally, never by a relying-party server); it exists to keep the
/// client-data JSON spec-shaped.
const ORIGIN: &str = "https://cyberdesk.local";
/// Modal-UI timeout for the Hello prompt (PIN entry included).
const TIMEOUT_MS: u32 = 120_000;
/// The minimum DLL API level for the salt path used here (see module docs:
/// the structs are v4-era; the pinned bindings are the v7 header).
const MIN_API_VERSION: u32 = 4;

/// Errors from the platform layer. `Cancelled` is the one flow-control case
/// (the user dismissed the Hello prompt); everything else carries the OS
/// error name - honest, and no oracle (no vault secret is involved yet).
#[derive(Debug)]
pub enum WebAuthnError {
    /// webauthn.dll reports an API level below the salt path's minimum.
    Unavailable(u32),
    /// The user dismissed / did not complete the Hello prompt.
    Cancelled,
    /// The OS call failed; `name` is WebAuthNGetErrorName's constant.
    Api { call: &'static str, name: String },
    /// The call succeeded but the response lacks what the vault needs
    /// (downlevel struct, no PRF output, unusable credential id).
    Response(String),
}

impl std::fmt::Display for WebAuthnError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WebAuthnError::Unavailable(v) => write!(
                f,
                "Windows WebAuthn is too old for the PRF salt path (API v{v}, needs v{MIN_API_VERSION})"
            ),
            WebAuthnError::Cancelled => write!(f, "Windows Hello was cancelled"),
            WebAuthnError::Api { call, name } => write!(f, "{call} failed: {name}"),
            WebAuthnError::Response(m) => write!(f, "{m}"),
        }
    }
}

type Result<T> = std::result::Result<T, WebAuthnError>;

/// A freshly enrolled Hello credential: the (non-secret) credential id the
/// vault persists, and the PRF-derived method secret in locked memory.
pub struct EnrolledPasskey {
    pub cred_id: Vec<u8>,
    pub secret: SecretBuf,
}

/// The installed webauthn.dll API level. The binding is raw-dylib
/// (load-time), so on the Windows-11-only target this always answers; the
/// number gates the salt path and feeds the honest config surface.
pub fn api_version() -> u32 {
    unsafe { WebAuthNGetApiVersionNumber() }
}

/// Is the PRF salt path available on this system? (Capability of the DLL;
/// whether HELLO itself serves hmac-secret is proven empirically at enroll.)
pub fn available() -> bool {
    api_version() >= MIN_API_VERSION
}

/// Is a user-verifying PLATFORM authenticator (Windows Hello with a PIN,
/// fingerprint or face enrolled) available right now? A live machine fact:
/// with attachment = PLATFORM, MakeCredential fails as NotSupportedError
/// when this is false (the CD-44 A3 finding, isolated by probe on the
/// target). Non-modal, cheap; consulted per state push so the config
/// surface updates itself once Hello is set up.
pub fn hello_ready() -> bool {
    let mut avail: BOOL = 0;
    let hr = unsafe { WebAuthNIsUserVerifyingPlatformAuthenticatorAvailable(&mut avail) };
    hr == 0 && avail != 0
}

/// The plain-language next step when Hello is not set up. Used by the vault
/// layer both as the pre-check refusal and to explain a NotSupportedError
/// that slips through (a status display must never lie, and a raw API error
/// name is not an explanation).
pub const HELLO_SETUP_HINT: &str = "Windows Hello is not set up on this device. \
Set up a PIN, fingerprint or face in Windows Settings > Accounts > Sign-in options, \
then add the passkey.";

// --- small marshalling helpers ---------------------------------------------

/// UTF-16, NUL-terminated, for PCWSTR fields. The buffer must outlive the
/// call that receives the pointer.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read_wide(p: *const u16) -> String {
    if p.is_null() {
        return "unknown error".into();
    }
    let mut len = 0usize;
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

/// Spec-shaped client-data JSON. The challenge is fresh randomness encoded
/// as hex - nothing ever verifies this assertion (see module docs), the
/// field exists to keep the structure honest and well-formed.
fn client_data_json(typ: &str) -> Result<String> {
    let mut challenge = [0u8; 32];
    getrandom::fill(&mut challenge)
        .map_err(|e| WebAuthnError::Response(format!("csprng failed: {e}")))?;
    let hex: String = challenge.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{{\"type\":\"{typ}\",\"challenge\":\"{hex}\",\"origin\":\"{ORIGIN}\",\"crossOrigin\":false}}"
    ))
}

fn client_data(json: &str) -> WEBAUTHN_CLIENT_DATA {
    WEBAUTHN_CLIENT_DATA {
        dwVersion: WEBAUTHN_CLIENT_DATA_CURRENT_VERSION,
        cbClientDataJSON: json.len() as u32,
        pbClientDataJSON: json.as_ptr() as *mut u8,
        pwszHashAlgId: WEBAUTHN_HASH_ALGORITHM_SHA_256,
    }
}

/// Map an HRESULT failure through WebAuthNGetErrorName into plain language
/// (CD-44 A3: a raw API error name is never the message a user sees).
/// "NotAllowedError" is the W3C name webauthn.dll returns for a
/// dismissed/timed-out prompt; "NotSupportedError" with no Hello enrolled is
/// the no-platform-authenticator case isolated on the target machine.
fn api_error(call: &'static str, hr: i32) -> WebAuthnError {
    let name = read_wide(unsafe { WebAuthNGetErrorName(hr) });
    match name.as_str() {
        "NotAllowedError" => WebAuthnError::Cancelled,
        "NotSupportedError" if !hello_ready() => {
            WebAuthnError::Response(HELLO_SETUP_HINT.into())
        }
        "NotSupportedError" => WebAuthnError::Response(format!(
            "Windows reported the passkey request as not supported on this device \
             (webauthn.dll API v{}). Check that Windows Hello works in Windows \
             Settings, then try again.",
            api_version()
        )),
        "InvalidStateError" => WebAuthnError::Response(
            "Windows Hello reports a matching passkey already exists on this device. \
             Remove the old CyberDesk entry under Windows Settings > Accounts > Passkeys, \
             then try again."
                .into(),
        ),
        "ConstraintError" => WebAuthnError::Response(
            "Windows Hello could not verify you (PIN, fingerprint or face). \
             Check the Hello sign-in options in Windows Settings, then try again."
                .into(),
        ),
        _ => WebAuthnError::Api { call, name },
    }
}

// --- enroll -----------------------------------------------------------------

/// Create the vault's Hello credential (user presence + verification via
/// PIN/fingerprint/face) with the hmac-secret capability, then run the first
/// PRF evaluation to derive the method secret. Two Hello prompts, by CTAP
/// design: hmac-secret output only exists at assertion time, and the v8
/// create-time eval is exactly the convenience this build does not depend on
/// (D-0063). BLOCKING + modal - vault worker threads only.
pub fn enroll(hwnd: isize, salt: &[u8; KEY_LEN]) -> Result<EnrolledPasskey> {
    let v = api_version();
    if v < MIN_API_VERSION {
        return Err(WebAuthnError::Unavailable(v));
    }

    let rp_id = wide(RP_ID);
    let rp_name = wide(RP_NAME);
    let user_name = wide("CyberDesk Vault");
    // The user handle: random, non-secret, persisted nowhere - each
    // enrollment is a fresh credential (one passkey max; re-enrolling after
    // a removal mints a new credential and the old OS entry is deleted
    // best-effort by the caller).
    let mut user_id = [0u8; 16];
    getrandom::fill(&mut user_id)
        .map_err(|e| WebAuthnError::Response(format!("csprng failed: {e}")))?;

    let rp = WEBAUTHN_RP_ENTITY_INFORMATION {
        dwVersion: WEBAUTHN_RP_ENTITY_INFORMATION_CURRENT_VERSION,
        pwszId: rp_id.as_ptr(),
        pwszName: rp_name.as_ptr(),
        pwszIcon: std::ptr::null(),
    };
    let user = WEBAUTHN_USER_ENTITY_INFORMATION {
        dwVersion: WEBAUTHN_USER_ENTITY_INFORMATION_CURRENT_VERSION,
        cbId: user_id.len() as u32,
        pbId: user_id.as_mut_ptr(),
        pwszName: user_name.as_ptr(),
        pwszIcon: std::ptr::null(),
        pwszDisplayName: user_name.as_ptr(),
    };

    // ES256 first (universally supported), RS256 as the fallback some TPMs
    // prefer. The credential's signature algorithm is irrelevant to the PRF.
    let mut cose_params = [
        WEBAUTHN_COSE_CREDENTIAL_PARAMETER {
            dwVersion: WEBAUTHN_COSE_CREDENTIAL_PARAMETER_CURRENT_VERSION,
            pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
            lAlg: WEBAUTHN_COSE_ALGORITHM_ECDSA_P256_WITH_SHA256,
        },
        WEBAUTHN_COSE_CREDENTIAL_PARAMETER {
            dwVersion: WEBAUTHN_COSE_CREDENTIAL_PARAMETER_CURRENT_VERSION,
            pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
            lAlg: WEBAUTHN_COSE_ALGORITHM_RSASSA_PKCS1_V1_5_WITH_SHA256,
        },
    ];
    let cose = WEBAUTHN_COSE_CREDENTIAL_PARAMETERS {
        cCredentialParameters: cose_params.len() as u32,
        pCredentialParameters: cose_params.as_mut_ptr(),
    };

    let json = client_data_json("webauthn.create")?;
    let cd = client_data(&json);

    // The classic create-time enable: the "hmac-secret" extension with a
    // BOOL payload (BOOL in / BOOL out per webauthn.h), PLUS the v6
    // `bEnablePrf` field - both express the same CTAP capability; setting
    // both matches what PRF-requesting browsers negotiate.
    let mut enable: BOOL = 1;
    let mut ext = [WEBAUTHN_EXTENSION {
        pwszExtensionIdentifier: WEBAUTHN_EXTENSIONS_IDENTIFIER_HMAC_SECRET,
        cbExtension: std::mem::size_of::<BOOL>() as u32,
        pvExtension: (&mut enable as *mut BOOL).cast(),
    }];

    // Everything not named is zero/null (the Default is `mem::zeroed`), and
    // the version stamp is the v7 CURRENT constant of the pinned bindings.
    let opts = WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS {
        dwVersion: WEBAUTHN_AUTHENTICATOR_MAKE_CREDENTIAL_OPTIONS_CURRENT_VERSION,
        dwTimeoutMilliseconds: TIMEOUT_MS,
        Extensions: WEBAUTHN_EXTENSIONS { cExtensions: 1, pExtensions: ext.as_mut_ptr() },
        dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM,
        dwUserVerificationRequirement: WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED,
        dwAttestationConveyancePreference: WEBAUTHN_ATTESTATION_CONVEYANCE_PREFERENCE_NONE,
        bEnablePrf: 1,
        ..Default::default()
    };

    let mut attestation: *mut WEBAUTHN_CREDENTIAL_ATTESTATION = std::ptr::null_mut();
    let hr = unsafe {
        WebAuthNAuthenticatorMakeCredential(
            hwnd as HWND,
            &rp,
            &user,
            &cose,
            &cd,
            &opts,
            &mut attestation,
        )
    };
    if hr != 0 {
        return Err(api_error("WebAuthNAuthenticatorMakeCredential", hr));
    }
    if attestation.is_null() {
        return Err(WebAuthnError::Response(
            "credential creation returned nothing".into(),
        ));
    }

    // Read gated on the RETURNED dwVersion (never past it - issue #262
    // lesson): the credential id is a v1 field; bPrfEnabled is v5+ and only
    // informational here (the eval assertion below is the real proof). The
    // id buffer is null-checked BEFORE slicing - an absent Win32 buffer is
    // NULL + zero count, and `from_raw_parts(NULL, 0)` is library UB.
    let cred_id;
    let prf_acked;
    unsafe {
        let a = &*attestation;
        cred_id = if a.pbCredentialId.is_null() || a.cbCredentialId == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(a.pbCredentialId, a.cbCredentialId as usize).to_vec()
        };
        prf_acked = if a.dwVersion >= 5 { a.bPrfEnabled != 0 } else { false };
        WebAuthNFreeCredentialAttestation(attestation);
    }
    if cred_id.is_empty() {
        return Err(WebAuthnError::Response("credential id was empty".into()));
    }
    if !prf_acked {
        tracing::warn!(
            "hello: create-time PRF ack absent (attestation downlevel or FALSE) - \
             the eval assertion decides"
        );
    }

    // First PRF evaluation - the authoritative capability check AND the
    // method secret in one step. If it fails (cancelled second prompt, or a
    // Hello build without hmac-secret), the just-created credential is
    // deleted best-effort: a failed enrollment must not accumulate orphaned
    // passkeys in Windows Settings - no half-enrolled state, OS side either.
    let secret = match assert(hwnd, &cred_id, salt) {
        Ok(s) => s,
        Err(e) => {
            delete_platform_credential(&cred_id);
            return Err(e);
        }
    };
    tracing::info!("hello: credential enrolled, PRF secret derived (api v{v})");
    Ok(EnrolledPasskey { cred_id, secret })
}

// --- assert (the PRF evaluation) --------------------------------------------

/// Run the Hello assertion for `cred_id` and derive the PRF secret for
/// `salt` (the stored per-passkey eval value; the DLL applies the WebAuthn
/// PRF spec hashing - see module docs). BLOCKING + modal - vault worker
/// threads only. The secret lands directly in locked memory; the DLL's
/// output buffer is zeroized before it is freed.
pub fn assert(hwnd: isize, cred_id: &[u8], salt: &[u8; KEY_LEN]) -> Result<SecretBuf> {
    let v = api_version();
    if v < MIN_API_VERSION {
        return Err(WebAuthnError::Unavailable(v));
    }
    if cred_id.is_empty() {
        return Err(WebAuthnError::Response("missing credential id".into()));
    }

    let rp_id = wide(RP_ID);
    let json = client_data_json("webauthn.get")?;
    let cd = client_data(&json);

    // Allowlist: exactly the enrolled credential.
    let mut id_buf = cred_id.to_vec();
    let mut cred = WEBAUTHN_CREDENTIAL_EX {
        dwVersion: WEBAUTHN_CREDENTIAL_EX_CURRENT_VERSION,
        cbId: id_buf.len() as u32,
        pbId: id_buf.as_mut_ptr(),
        pwszCredentialType: WEBAUTHN_CREDENTIAL_TYPE_PUBLIC_KEY,
        dwTransports: 0, // no transport restriction - the id pins it
    };
    let mut cred_ptr: *mut WEBAUTHN_CREDENTIAL_EX = &mut cred;
    let mut allow = WEBAUTHN_CREDENTIAL_LIST { cCredentials: 1, ppCredentials: &mut cred_ptr };

    // The PRF eval value ("first" slot only). dwFlags deliberately does NOT
    // include WEBAUTHN_AUTHENTICATOR_HMAC_SECRET_VALUES_FLAG: without it the
    // DLL converts this value per the WebAuthn PRF spec -
    // SHA-256("WebAuthn PRF" || 0x00 || value) - before CTAP (webauthn.h).
    let mut salt_buf = *salt;
    let mut prf_salt = WEBAUTHN_HMAC_SECRET_SALT {
        cbFirst: salt_buf.len() as u32,
        pbFirst: salt_buf.as_mut_ptr(),
        cbSecond: 0,
        pbSecond: std::ptr::null_mut(),
    };
    let mut salt_values = WEBAUTHN_HMAC_SECRET_SALT_VALUES {
        pGlobalHmacSalt: &mut prf_salt,
        cCredWithHmacSecretSaltList: 0,
        pCredWithHmacSecretSaltList: std::ptr::null_mut(),
    };

    // Everything not named is zero/null; notably dwFlags stays 0 (no RAW
    // flag - the DLL applies the WebAuthn-PRF spec hashing, see above).
    let opts = WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS {
        dwVersion: WEBAUTHN_AUTHENTICATOR_GET_ASSERTION_OPTIONS_CURRENT_VERSION,
        dwTimeoutMilliseconds: TIMEOUT_MS,
        dwAuthenticatorAttachment: WEBAUTHN_AUTHENTICATOR_ATTACHMENT_PLATFORM,
        dwUserVerificationRequirement: WEBAUTHN_USER_VERIFICATION_REQUIREMENT_REQUIRED,
        pAllowCredentialList: &mut allow,
        pHmacSecretSaltValues: &mut salt_values,
        ..Default::default()
    };

    let mut assertion: *mut WEBAUTHN_ASSERTION = std::ptr::null_mut();
    let hr = unsafe {
        WebAuthNAuthenticatorGetAssertion(hwnd as HWND, rp_id.as_ptr(), &cd, &opts, &mut assertion)
    };
    // The eval value's stack copy is done with either way.
    salt_buf.zeroize();
    if hr != 0 {
        return Err(api_error("WebAuthNAuthenticatorGetAssertion", hr));
    }
    if assertion.is_null() {
        return Err(WebAuthnError::Response("assertion returned nothing".into()));
    }

    // Extract the PRF output - reads gated on the RETURNED dwVersion
    // (pHmacSecret is a v3+ field; a downlevel struct means the OS did not
    // evaluate hmac-secret at all). WHATEVER the outcome, any PRF bytes the
    // DLL handed back are zeroized in place before the free - the wipe runs
    // on every path where pHmacSecret is non-null, not just success
    // (bounded residual: internal copies beyond this pointer are the DLL's /
    // broker's - documented in cyberdesk-security.md).
    let result = unsafe {
        let a = &*assertion;
        if a.dwVersion < WEBAUTHN_ASSERTION_VERSION_3 {
            Err(WebAuthnError::Response(format!(
                "this Windows build returned a downlevel assertion (v{}) without hmac-secret output",
                a.dwVersion
            )))
        } else if a.pHmacSecret.is_null() {
            Err(WebAuthnError::Response(
                "Windows Hello returned no PRF output for this credential - \
                 the credential lacks hmac-secret, or this Windows build predates \
                 Hello PRF support (KB5077181)"
                    .into(),
            ))
        } else {
            let out = &mut *a.pHmacSecret;
            let first_len = out.cbFirst as usize;
            let secret = if first_len != KEY_LEN || out.pbFirst.is_null() {
                Err(WebAuthnError::Response(format!(
                    "unexpected PRF output length {}",
                    out.cbFirst
                )))
            } else {
                SecretBuf::copy_of(std::slice::from_raw_parts(out.pbFirst, first_len))
                    .map_err(|e| WebAuthnError::Response(e.to_string()))
            };
            if !out.pbFirst.is_null() && first_len > 0 {
                std::slice::from_raw_parts_mut(out.pbFirst, first_len).zeroize();
            }
            if !out.pbSecond.is_null() && out.cbSecond > 0 {
                std::slice::from_raw_parts_mut(out.pbSecond, out.cbSecond as usize).zeroize();
            }
            secret
        }
    };
    unsafe { WebAuthNFreeAssertion(assertion) };
    result
}

// --- removal cleanup --------------------------------------------------------

/// Best-effort deletion of the OS-side platform credential when the vault
/// passkey is removed (the vault's envelope state is already authoritative;
/// this only keeps Windows' credential list tidy). Errors are logged, never
/// surfaced - the vault removal has already succeeded.
pub fn delete_platform_credential(cred_id: &[u8]) {
    if cred_id.is_empty() {
        return;
    }
    let hr = unsafe { WebAuthNDeletePlatformCredential(cred_id.len() as u32, cred_id.as_ptr()) };
    if hr != 0 {
        tracing::info!(
            "hello: platform-credential cleanup skipped: {}",
            read_wide(unsafe { WebAuthNGetErrorName(hr) })
        );
    }
}

// ---------------------------------------------------------------------------
// Tests - headless half (Task E): the FFI links against the OS DLL and the
// pure marshalling helpers behave. The modal enroll/assert calls need a user
// gesture and are the maintainer's live check by design.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The raw-dylib binding resolves against the OS webauthn.dll and the
    /// API level supports the v4-era salt path (Windows-11-only target).
    #[test]
    fn webauthn_dll_links_and_api_level_suffices() {
        let v = api_version();
        assert!(
            v >= MIN_API_VERSION,
            "webauthn.dll API v{v} below the salt-path minimum {MIN_API_VERSION}"
        );
        assert!(available());
    }

    /// The error-name plumbing round-trips a real HRESULT through the DLL.
    #[test]
    fn error_names_resolve() {
        // NTE_EXISTS (0x8009000F) is what a duplicate credential raises;
        // any HRESULT yields SOME stable name string.
        let name = read_wide(unsafe { WebAuthNGetErrorName(0x8009000Fu32 as i32) });
        assert!(!name.is_empty());
    }

    #[test]
    fn wide_strings_are_nul_terminated_utf16() {
        let w = wide("cyberdesk.local");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(String::from_utf16_lossy(&w[..w.len() - 1]), "cyberdesk.local");
        assert_eq!(read_wide(w.as_ptr()), "cyberdesk.local");
        assert_eq!(read_wide(std::ptr::null()), "unknown error");
    }

    /// Client data stays spec-shaped and fresh per call (the challenge is
    /// random; nothing may reuse one).
    #[test]
    fn client_data_json_is_well_formed_and_fresh() {
        let a = client_data_json("webauthn.create").unwrap();
        let b = client_data_json("webauthn.create").unwrap();
        assert_ne!(a, b, "fresh challenge per call");
        let v: serde_json::Value = serde_json::from_str(&a).expect("valid JSON");
        assert_eq!(v["type"], "webauthn.create");
        assert_eq!(v["origin"], ORIGIN);
        assert_eq!(v["challenge"].as_str().unwrap().len(), 64, "32 random bytes, hex");
        let cd = client_data(&a);
        assert_eq!(cd.dwVersion, WEBAUTHN_CLIENT_DATA_CURRENT_VERSION);
        assert_eq!(cd.cbClientDataJSON as usize, a.len());
    }
}
