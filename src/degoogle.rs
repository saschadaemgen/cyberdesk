//! De-Google enforcement table (CD-17, D-0041; extended CD-26, D-0042).
//!
//! CyberDesk runs a full Chromium (via CEF 149). After CD-24/D-0036 the HOST
//! opens no HTTP client of its own, but the Chromium engine underneath still
//! phones Google home by default at many points (Safe Browsing, the component
//! updater, variations/Finch, connectivity probes, network prediction, search
//! suggest, domain reliability/NEL, translate, spell check, autofill, the
//! password leak-check, secure-DNS auto-upgrade, optimization hints, GCM,
//! and - found by the CD-26 idle re-audit - the eager signin ListAccounts
//! poll and the AI-Mode eligibility fetch). This module is the single,
//! auditable TABLE of every vector we close and HOW - command-line switches
//! for the process-global levers, preferences for the per-profile ones, one
//! global (local-state) preference for secure DNS.
//!
//! **Every switch and preference NAME here was verified against the pinned
//! Chromium `149.0.7827.201` source (CEF `149.0.6`), not guessed** - the
//! `source:` field on each entry cites the defining file. The application code
//! (the CEF calls) lives in `browser.rs`; this module is deliberately pure data
//! plus the one bit of testable logic (the `--disable-features` merge), so the
//! enforced set can be reviewed and unit-tested in one place.
//!
//! Honest bound (D-0041): this silences the engine's UNSOLICITED Google/telemetry
//! traffic. It does NOT hide the user's own navigation, and it does not disable
//! necessary TLS security (certificate verification stays on - OCSP/CRL to a
//! visited site's own CA is necessary infrastructure, not phone-home).

/// A preference value to force. The three variants map onto CEF's
/// `CefValue::SetBool` / `SetInt` / `SetString`.
#[derive(Clone, Copy, Debug)]
pub enum PrefValue {
    Bool(bool),
    Int(i32),
    Str(&'static str),
}

/// One forced preference, with its Chromium-source citation and the concrete
/// phone-home traffic it closes (so the net-log audit can attribute each entry).
#[derive(Clone, Copy, Debug)]
pub struct Pref {
    /// The Chromium preference path (e.g. `safebrowsing.enabled`).
    pub name: &'static str,
    /// The value we pin it to.
    pub value: PrefValue,
    /// Chromium 149.0.7827.201 file defining the pref-name constant.
    pub source: &'static str,
    /// The Google/telemetry connection this closes.
    pub closes: &'static str,
}

/// PROFILE preferences, applied to EVERY request context - the global (clearnet)
/// context in `BrowserProcessHandler::on_context_initialized` and each per-slot
/// Tor context in `TorContextHandler::on_request_context_initialized` - so
/// clearnet and Tor slots are de-Googled alike (CD-17 §1).
///
/// These are all user-modifiable profile prefs registered by Chromium, settable
/// via `CefRequestContext::SetPreference`. Each application is logged; a name that
/// ever fails to apply surfaces as an error in the rolling log (never a silent
/// no-op - the CD-15-HOTFIX lesson).
pub const CONTEXT_PREFS: &[Pref] = &[
    Pref {
        name: "safebrowsing.enabled",
        value: PrefValue::Bool(false),
        source: "components/safe_browsing/core/common/safe_browsing_prefs.h",
        closes: "Safe Browsing per-navigation URL/hash lookups + client-side phishing model fetch",
    },
    Pref {
        name: "safebrowsing.scout_reporting_enabled",
        value: PrefValue::Bool(false),
        source: "components/safe_browsing/core/common/safe_browsing_prefs.h",
        closes: "Safe Browsing extended (Scout) telemetry reporting",
    },
    Pref {
        name: "search.suggest_enabled",
        value: PrefValue::Bool(false),
        source: "chrome/common/pref_names.h",
        closes: "omnibox/search 'suggest' queries to the default search provider",
    },
    Pref {
        name: "alternate_error_pages.enabled",
        value: PrefValue::Bool(false),
        source: "components/embedder_support/pref_names.h",
        closes: "navigation-error 'did you mean' / Link-Doctor lookups",
    },
    Pref {
        // NetworkPredictionOptions enum: 0 = ALWAYS, 1 = WIFI_ONLY (default),
        // 2 = NEVER. Pin NEVER so nothing is speculatively resolved/preconnected.
        name: "net.network_prediction_options",
        value: PrefValue::Int(2),
        source: "chrome/common/pref_names.h",
        closes: "network prediction / speculative DNS-preresolve + preconnect",
    },
    Pref {
        name: "spellcheck.use_spelling_service",
        value: PrefValue::Bool(false),
        source: "components/spellcheck/browser/pref_names.h",
        closes: "enhanced spell check (typed text sent to the Google spelling service)",
    },
    Pref {
        name: "translate.enabled",
        value: PrefValue::Bool(false),
        source: "components/translate/core/browser/translate_pref_names.h",
        closes: "translate service (page text + language sent to Google Translate)",
    },
    Pref {
        name: "profile.password_manager_leak_detection",
        value: PrefValue::Bool(false),
        source: "components/password_manager/core/common/password_manager_pref_names.h",
        closes: "password leak-detection check to Google on sign-in",
    },
    Pref {
        name: "autofill.profile_enabled",
        value: PrefValue::Bool(false),
        source: "components/autofill/core/common/autofill_prefs.h",
        closes: "Autofill address crowdsourcing queries/uploads to Google",
    },
    Pref {
        name: "autofill.credit_card_enabled",
        value: PrefValue::Bool(false),
        source: "components/autofill/core/common/autofill_prefs.h",
        closes: "Autofill credit-card + payments queries to Google",
    },
];

/// GLOBAL (local-state) preferences, set through the global
/// `CefPreferenceManager` and guarded by `CanSetPreference` - these live in
/// local state, not the profile, so `CefRequestContext::SetPreference` can't
/// reach them.
///
/// Secure DNS: pin the DoH mode to `off` so clearnet uses the plain OS resolver
/// DETERMINISTICALLY rather than Chromium's default `automatic` auto-upgrade
/// (which could route DNS to a DoH provider). Tor slots resolve DNS remotely
/// through the SOCKS tunnel regardless (CD-15), so this only governs clearnet.
/// SecureDnsMode string values are `off` / `automatic` / `secure`.
pub const GLOBAL_PREFS: &[Pref] = &[Pref {
    name: "dns_over_https.mode",
    value: PrefValue::Str("off"),
    source: "chrome/common/pref_names.h (registered in local state)",
    closes: "automatic secure-DNS/DoH resolver auto-upgrade on clearnet",
}];

/// Boolean process-global command-line switches, appended in
/// `App::on_before_command_line_processing`. Applied for EVERY process (a feature
/// or behaviour toggle must agree browser<->renderer). `disable-quic` (CD-15,
/// D-0027) is appended separately and stays.
///
/// `disable-background-networking` is the UMBRELLA - in Chromium 149 it gates the
/// component-updater fetch, the variations/Finch seed fetch, GCM/push, the Safe
/// Browsing update fetch, and more. It does NOT cover the per-navigation levers
/// (Safe Browsing lookups, search suggest, translate, …) - those are closed
/// explicitly above. The remaining switches are belt-and-suspenders over the
/// umbrella (component updater, NEL beacons, account sync).
pub const SWITCHES: &[&str] = &[
    // kDisableBackgroundNetworking = "disable-background-networking"
    // - chrome/common/chrome_switches.cc
    "disable-background-networking",
    // kDisableComponentUpdate = "disable-component-update"
    // - chrome/common/chrome_switches.cc (CRLSet, Widevine, … from Google)
    "disable-component-update",
    // kDisableDomainReliability = "disable-domain-reliability"
    // - chrome/common/chrome_switches.cc (Domain Reliability / NEL beacons)
    "disable-domain-reliability",
    // kDisableSync = "disable-sync"
    // - components/sync/base/command_line_switches.h (Google account sync)
    "disable-sync",
];

/// A process-global command-line switch that carries a VALUE (`--name=value`),
/// with the same citation discipline as [`Pref`]: the Chromium file defining
/// the switch and the concrete traffic the setting closes.
#[derive(Clone, Copy, Debug)]
pub struct ValuedSwitch {
    /// Bare switch name (no leading dashes).
    pub name: &'static str,
    /// The value we pin.
    pub value: &'static str,
    /// Chromium 149.0.7827.201 file defining/consuming the switch.
    pub source: &'static str,
    /// The Google/telemetry connection this closes.
    pub closes: &'static str,
}

/// Valued switches, appended in `App::on_before_command_line_processing`
/// alongside [`SWITCHES`] (CD-26, D-0042).
///
/// Background (CD-26): the idle net-log still showed one deterministic
/// `POST accounts.google.com/ListAccounts` ~90 ms after startup. Its net-log
/// traffic annotation (`gaia_auth_list_accounts`) traces to `AccountInvestigator`
/// - a per-profile KeyedService that Chromium creates EAGERLY
/// (`AccountInvestigatorFactory::ServiceIsCreatedWithBrowserContext() == true`,
/// chrome/browser/signin/account_investigator_factory.cc) and whose persistent
/// daily timer fires IMMEDIATELY on a fresh or stale profile
/// (components/signin/public/base/persistent_repeating_timer.cc:
/// `if (desired_run_time <= clock_->Now()) { OnTimerFired(); }`). That path
/// checks NO preference, NO policy and NO feature - it cannot be disabled,
/// only redirected. Hence `gaia-url`: every GAIA endpoint URL is derived from
/// one origin that this switch overrides, so the whole signin endpoint set
/// (ListAccounts/Logout/multilogin) resolves to a dead loopback origin.
/// TCP RST on 127.0.0.1:9 (discard port, nothing listens); zero bytes leave
/// the machine. The audit may see a few refused loopback attempts (bounded
/// GaiaCookieManagerService backoff) - that is the neutered stack, documented
/// in docs/cyberdesk-degoogle-audit.md.
pub const VALUED_SWITCHES: &[ValuedSwitch] = &[
    ValuedSwitch {
        // kAllowBrowserSigninArgument = "allow-browser-signin"; parsed by
        // IsBrowserSigninAllowedByCommandLine - only the literal "true"
        // enables, so "=false" pins signin.allowed=false at every profile
        // init → AccountConsistencyModeManager returns kDisabled → the
        // AccountReconcilor gets the base delegate whose IsReconcileEnabled()
        // is false and never lists accounts.
        name: "allow-browser-signin",
        value: "false",
        source: "chrome/browser/signin/account_consistency_mode_manager.cc",
        closes: "browser signin / DICE account consistency - every reconcilor-driven \
                 accounts.google.com ListAccounts (token/cookie-change triggered)",
    },
    ValuedSwitch {
        // kGaiaUrl = "gaia-url" - google_apis/gaia/gaia_switches.cc; consumed
        // once in GaiaUrls::InitializeDefault (google_apis/gaia/gaia_urls.cc):
        // SetDefaultOriginIfOpaqueOrInvalidScheme(&gaia_origin_, kGaiaUrl, …)
        // then list_accounts_url_ etc. are all resolved against that origin.
        // Loopback discard port: connection refused locally, never routed.
        name: "gaia-url",
        value: "http://127.0.0.1:9/",
        source: "google_apis/gaia/gaia_switches.cc",
        closes: "the ENTIRE GAIA endpoint set - above all AccountInvestigator's eager \
                 startup/daily ListAccounts POST, which no pref/policy/feature gates",
    },
];

/// Feature names merged into `--disable-features`. `OptimizationHints`
/// (`kOptimizationHints` = "OptimizationHints",
/// components/optimization_guide/core/optimization_guide_features.cc) fetches
/// page-optimization hints from Google.
///
/// NOTE: the field-trial TESTING config is already disabled in official CEF
/// builds (`disable_fieldtrial_testing_config=true`, per cef_preference.h), so no
/// extra switch is needed to keep Finch trials inert.
///
/// All two-arg `BASE_FEATURE(kFoo, …)` macros derive the runtime string from
/// the identifier (base/feature.h), so each name below is the exact
/// `--disable-features` token.
pub const DISABLE_FEATURES: &[&str] = &[
    "OptimizationHints",
    // CD-26 (D-0042): Google AI-Mode eligibility. AimEligibilityService is
    // created EAGERLY with every profile
    // (chrome/browser/autocomplete/aim_eligibility_service_factory.cc) and its
    // constructor fires `GET www.google.com/async/folae?…udm=50…` at startup
    // (net-log annotation `aim_eligibility_fetch`), sending google cookies AND
    // the x-client-data variations header. `kAimEnabled`
    // (components/omnibox/browser/aim_eligibility_service_features.cc) is the
    // umbrella: the service constructor returns before registering ANY
    // observer or request when it is disabled, and IsAimLocallyEligible()
    // documents it as the kill switch.
    "AimEnabled",
    // Belt-and-suspenders under the AimEnabled umbrella: the startup-fired
    // request (the one the idle audit caught) and the identity-change refetch,
    // each individually gated in aim_eligibility_service.cc.
    "AimServerRequestOnStartupEnabled",
    "AimServerRequestOnIdentityChangeEnabled",
    // CD-26 (D-0042): the GENERIC Reporting API + Network Error Logging -
    // NOT covered by `disable-domain-reliability` (a different feature). Both
    // default-enabled (`kReporting`, `kNetworkErrorLogging`,
    // services/network/public/cpp/features.cc) and gated per network context
    // in NetworkContext::MakeURLRequestContext, so disabling stops background
    // report delivery AND stops the persisted "<profile>/Network/Reporting
    // and NEL" endpoint store (observed carrying 10 csp.withgoogle.com
    // registrations from a past google.com visit) from being loaded at all.
    "Reporting",
    "NetworkErrorLogging",
];

/// Env var (set to a writable path) that turns on the net-log capture used for
/// the CD-17 §2 audit. OFF by default - nothing lands on disk in a normal run
/// (anti-forensic tenet). Read in `on_before_command_line_processing`, which then
/// appends `--log-net-log=<path>` on the browser process only.
pub const AUDIT_NETLOG_ENV: &str = "CYBERDESK_AUDIT_NETLOG";

/// Merge our [`DISABLE_FEATURES`] into an existing `--disable-features` value
/// (comma-separated), preserving the existing entries and their order and adding
/// each of ours only if absent. Idempotent: merging an already-merged value is a
/// no-op. Returns `""` only when there is nothing to disable at all.
///
/// This matters because `base::CommandLine` stores switches in a map - appending
/// a second `--disable-features` would CLOBBER whatever CEF/Chromium already put
/// there, so we read, merge, and re-set instead.
pub fn merge_disable_features(existing: &str) -> String {
    let mut out: Vec<&str> = existing
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    for f in DISABLE_FEATURES {
        if !out.contains(f) {
            out.push(f);
        }
    }
    out.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The full canonical feature list as one `--disable-features` value.
    fn ours() -> String {
        DISABLE_FEATURES.join(",")
    }

    #[test]
    fn merge_into_empty_is_just_ours() {
        assert_eq!(merge_disable_features(""), ours());
        assert_eq!(merge_disable_features("   "), ours());
    }

    #[test]
    fn merge_preserves_existing_and_appends() {
        assert_eq!(
            merge_disable_features("Foo,Bar"),
            format!("Foo,Bar,{}", ours())
        );
    }

    #[test]
    fn merge_is_idempotent_and_dedups() {
        // One of ours already present anywhere in the list → not added twice.
        let merged = merge_disable_features("Foo,Reporting,Bar");
        assert_eq!(
            merged.split(',').filter(|f| *f == "Reporting").count(),
            1,
            "duplicated feature in {merged}"
        );
        // Merging our own output again changes nothing.
        let once = merge_disable_features("Foo");
        assert_eq!(merge_disable_features(&once), once);
        // The canonical list itself is a fixpoint.
        assert_eq!(merge_disable_features(&ours()), ours());
    }

    #[test]
    fn merge_trims_stray_whitespace() {
        assert_eq!(
            merge_disable_features(" Foo , Bar "),
            format!("Foo,Bar,{}", ours())
        );
    }

    #[test]
    fn tables_are_populated_and_well_formed() {
        // A regression guard: the enforced set must never silently empty out.
        assert!(!SWITCHES.is_empty());
        assert!(!VALUED_SWITCHES.is_empty());
        assert!(!CONTEXT_PREFS.is_empty());
        assert!(!GLOBAL_PREFS.is_empty());
        assert!(!DISABLE_FEATURES.is_empty());
        for p in CONTEXT_PREFS.iter().chain(GLOBAL_PREFS) {
            assert!(p.name.contains('.'), "pref name looks malformed: {}", p.name);
            assert!(!p.source.is_empty());
            assert!(!p.closes.is_empty());
        }
        for s in SWITCHES {
            assert!(!s.is_empty() && !s.starts_with('-'), "switch should be bare: {s}");
        }
        for s in VALUED_SWITCHES {
            assert!(
                !s.name.is_empty() && !s.name.starts_with('-'),
                "switch should be bare: {}",
                s.name
            );
            assert!(!s.value.is_empty(), "valued switch without value: {}", s.name);
            assert!(!s.source.is_empty());
            assert!(!s.closes.is_empty());
        }
        // The feature table itself stays canonical (the merge dedups anyway).
        let mut seen = std::collections::HashSet::new();
        for f in DISABLE_FEATURES {
            assert!(seen.insert(f), "duplicate feature: {f}");
        }
    }

    #[test]
    fn gaia_redirect_stays_on_loopback() {
        // The gaia-url override must NEVER point at a routable origin - the
        // point of the dead origin is that the neutered GAIA stack cannot
        // leave the machine (CD-26, D-0042).
        let gaia = VALUED_SWITCHES
            .iter()
            .find(|s| s.name == "gaia-url")
            .expect("gaia-url redirect missing from VALUED_SWITCHES");
        assert!(
            gaia.value.starts_with("http://127.0.0.1:"),
            "gaia-url must stay loopback: {}",
            gaia.value
        );
    }
}
