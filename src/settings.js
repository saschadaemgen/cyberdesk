// Settings page logic. Talks to the Rust host exclusively over the CEF message
// router (window.cefQuery) — no network, no fetch, no external resources. The
// wire format is documented in docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  var statusEl = document.getElementById("status");

  function setStatus(text, isError) {
    statusEl.textContent = text || "";
    statusEl.classList.toggle("error", !!isError);
  }

  // Wrap a single cefQuery request/response as a promise.
  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) {
        reject("settings IPC unavailable");
        return;
      }
      window.cefQuery({
        request: JSON.stringify(req),
        persistent: false,
        onSuccess: function (response) { resolve(response); },
        onFailure: function (code, message) { reject(message || ("error " + code)); }
      });
    });
  }

  function switchEl(key) {
    return document.querySelector('.switch[data-key="' + key + '"]');
  }

  function paint(key, on) {
    var el = switchEl(key);
    if (!el) return;
    el.classList.toggle("on", !!on);
    el.setAttribute("aria-checked", on ? "true" : "false");
  }

  // Optimistically flip, persist via IPC, revert on failure.
  function toggle(el) {
    var key = el.dataset.key;
    var next = !el.classList.contains("on");
    // Turning OFF the on-launch residue purge weakens the anti-forensic guarantee
    // (residue would accumulate on disk), so it routes through the honest two-
    // confirmation gate (D-0040) — the host re-validates it too. Turning it back on
    // is immediate. Every other toggle is a plain flip.
    if (key === "purge_residue" && !next) {
      openGate({
        title: "Stop purging browsing residue?",
        body: "With this off, any browsing cache or content that reaches the disk " +
          "<strong>stays there</strong> and builds up across launches, so what you browsed " +
          "could be recovered from the disk later. Browsing still runs in memory — but the " +
          "disk safety net is gone. Keep it on unless you have a specific reason."
      }, function () {
        paint(key, false);
        setStatus("");
        query({ cmd: "set_setting", key: key, value: false, confirm: true })
          .then(refreshResidue)
          .catch(function (err) { paint(key, true); setStatus(String(err), true); });
      });
      return;
    }
    paint(key, next);
    setStatus("");
    query({ cmd: "set_setting", key: key, value: next })
      .then(function () { if (key === "purge_residue") refreshResidue(); })
      .catch(function (err) {
        paint(key, !next);
        setStatus(String(err), true);
      });
  }

  // Only the generic settings toggles (data-key). The per-vector hardening switches
  // (data-fp) are wired separately (CD-25) because turning one off is gated.
  var switches = document.querySelectorAll(".switch[data-key]");
  for (var i = 0; i < switches.length; i++) {
    (function (el) {
      el.addEventListener("click", function () { toggle(el); });
      el.addEventListener("keydown", function (e) {
        if (e.key === " " || e.key === "Enter") { e.preventDefault(); toggle(el); }
      });
    })(switches[i]);
  }

  // Glow-intensity slider: applied live on every input, persisted host-side.
  var glow = document.getElementById("glow");
  var glowVal = document.getElementById("glow-val");

  function paintGlow(percent) {
    var min = parseInt(glow.min, 10);
    var max = parseInt(glow.max, 10);
    glow.value = percent;
    glowVal.textContent = percent + "%";
    var fill = ((percent - min) / (max - min)) * 100;
    glow.style.setProperty("--fill", fill + "%");
  }

  glow.addEventListener("input", function () {
    var percent = parseInt(glow.value, 10);
    paintGlow(percent);
    query({ cmd: "set_setting", key: "glow_intensity", value: percent })
      .catch(function (err) { setStatus(String(err), true); });
  });

  // Search-engine select: a custom in-page dropdown (CEF OSR does not paint
  // native <select> popups — see settings.css / D-0015). Applied live, persisted.
  var engineSelect = document.getElementById("engine-select");
  var engineBtn = document.getElementById("engine-btn");
  var engineMenu = document.getElementById("engine-menu");
  var engineVal = document.getElementById("engine-val");
  var ENGINE_LABELS = {
    google: "Google", duckduckgo: "DuckDuckGo", bing: "Bing",
    startpage: "Startpage", brave: "Brave Search"
  };

  function paintEngine(value) {
    // Unknown values paint the factory default, DuckDuckGo (CD-27, D-0043).
    var v = ENGINE_LABELS[value] ? value : "duckduckgo";
    engineVal.textContent = ENGINE_LABELS[v];
    var opts = engineMenu.querySelectorAll("li");
    for (var i = 0; i < opts.length; i++) {
      opts[i].setAttribute("aria-selected", opts[i].dataset.value === v ? "true" : "false");
    }
  }

  function openEngine(open) {
    engineSelect.classList.toggle("open", open);
    engineBtn.setAttribute("aria-expanded", open ? "true" : "false");
    engineMenu.hidden = !open;
  }

  engineBtn.addEventListener("click", function (e) {
    e.stopPropagation();
    openEngine(!engineSelect.classList.contains("open"));
  });

  var engineOpts = engineMenu.querySelectorAll("li");
  for (var i = 0; i < engineOpts.length; i++) {
    (function (li) {
      li.addEventListener("click", function (e) {
        e.stopPropagation();
        var value = li.dataset.value;
        paintEngine(value);
        openEngine(false);
        query({ cmd: "set_setting", key: "search_engine", value: value })
          .catch(function (err) { setStatus(String(err), true); });
      });
    })(engineOpts[i]);
  }

  // Identity rotation interval slider (CD-29): minutes between automatic rotations.
  var rotInterval = document.getElementById("rot-interval");
  var rotIntervalVal = document.getElementById("rot-interval-val");
  function paintRotInterval(minutes) {
    var m = parseInt(minutes, 10);
    if (isNaN(m)) m = 15;
    var min = parseInt(rotInterval.min, 10);
    var max = parseInt(rotInterval.max, 10);
    m = Math.max(min, Math.min(max, m));
    rotInterval.value = m;
    rotIntervalVal.textContent = m + " min";
    var fill = ((m - min) / (max - min)) * 100;
    rotInterval.style.setProperty("--fill", fill + "%");
  }
  rotInterval.addEventListener("input", function () {
    var m = parseInt(rotInterval.value, 10);
    paintRotInterval(m);
    query({ cmd: "set_setting", key: "rotate_interval_min", value: m })
      .catch(function (err) { setStatus(String(err), true); });
  });

  // Reported screen-size select (CD-29): a common real resolution for screen.*.
  // Same custom-dropdown pattern as the engine select; applied live, persisted.
  var screenSelect = document.getElementById("screen-select");
  var screenBtn = document.getElementById("screen-btn");
  var screenMenu = document.getElementById("screen-menu");
  var screenVal = document.getElementById("screen-val");
  var SCREEN_LABELS = { "1920x1080": "1920 × 1080", "1600x900": "1600 × 900", "1280x720": "1280 × 720" };

  function paintScreen(value) {
    var v = SCREEN_LABELS[value] ? value : "1920x1080";
    screenVal.textContent = SCREEN_LABELS[v];
    var opts = screenMenu.querySelectorAll("li");
    for (var i = 0; i < opts.length; i++) {
      opts[i].setAttribute("aria-selected", opts[i].dataset.value === v ? "true" : "false");
    }
  }
  function openScreen(open) {
    screenSelect.classList.toggle("open", open);
    screenBtn.setAttribute("aria-expanded", open ? "true" : "false");
    screenMenu.hidden = !open;
  }
  screenBtn.addEventListener("click", function (e) {
    e.stopPropagation();
    openScreen(!screenSelect.classList.contains("open"));
  });
  var screenOpts = screenMenu.querySelectorAll("li");
  for (var si = 0; si < screenOpts.length; si++) {
    (function (li) {
      li.addEventListener("click", function (e) {
        e.stopPropagation();
        var value = li.dataset.value;
        paintScreen(value);
        openScreen(false);
        query({ cmd: "set_setting", key: "screen_preset", value: value })
          .catch(function (err) { setStatus(String(err), true); });
      });
    })(screenOpts[si]);
  }

  // --- Fingerprinting-hardening controls (CD-25; Ampel-graded CD-30) -------
  // Global Ampel level (Off/Green/Yellow/Red/Custom) + a per-vector detail view,
  // with a two-confirmation gate on any WEAKENING (any step DOWN the ladder —
  // Red→Yellow→Green→Off — or a dropped vector; stepping UP is immediate). The
  // weaken classification mirrors harden.rs::is_weakening; the host re-validates
  // it, so the gate here is UX, not the trust boundary.
  var fpSelect = document.getElementById("fp-select");
  var fpBtn = document.getElementById("fp-btn");
  var fpMenu = document.getElementById("fp-menu");
  var fpVal = document.getElementById("fp-val");
  var fpLevelPill = document.getElementById("fp-level");
  var fpDetail = document.getElementById("fp-detail");
  var FP_LABELS = { off: "Off", green: "Green", yellow: "Yellow", red: "Red", custom: "Custom" };
  // Pre-CD-30 names arriving from an old store snapshot map onto the Ampel.
  function canonLevel(l) { return l === "standard" ? "yellow" : l === "strict" ? "red" : l; }
  // The full CD-29 vector list (canonical order matches harden.rs::VECTOR_KEYS).
  var VECTORS = ["canvas", "webgl", "gpu", "audio", "metrics", "nav", "fonts", "timing", "media",
    "math", "viewport"];
  // Green = the coherent everyday core: everything except the three aggressive
  // clamps below (mirror harden.rs::Config::GREEN).
  var GREEN_OFF = ["timing", "media", "math"];
  var VECTOR_LABELS = {
    canvas: "canvas", webgl: "WebGL readback", gpu: "GPU identity", audio: "audio",
    metrics: "layout & text metrics", nav: "device profile", fonts: "fonts",
    timing: "clock precision", media: "media & codecs", math: "math rounding"
  };
  function allVectors(on) {
    var o = {};
    VECTORS.forEach(function (k) { o[k] = on; });
    return o;
  }
  var fpState = { preset: "green", vectors: allVectors(true) };

  // Resolve a (preset, vectors) into the effective config for the weaken
  // classification (mirror harden.rs::resolve, incl. the Red `strict` buckets).
  function fpEffective(preset, vectors) {
    if (preset === "off") {
      var off = allVectors(false); off.on = false; off.strict = false; return off;
    }
    if (preset === "custom") {
      var eff = {}, any = false;
      VECTORS.forEach(function (k) { eff[k] = !!vectors[k]; if (vectors[k]) any = true; });
      eff.on = any; eff.strict = false;
      return eff;
    }
    var on = allVectors(true);
    if (preset === "green") { GREEN_OFF.forEach(function (k) { on[k] = false; }); }
    on.on = true;
    on.strict = preset === "red";
    return on;
  }
  var GREEN_EFF = fpEffective("green", null);
  function isWeakening(cur, tgt) {
    if (cur.on && !tgt.on) return true;
    // CD-30: leaving the tight Red buckets is a weakening (the ladder is ordered).
    if (cur.on && cur.strict && !tgt.strict) return true;
    for (var i = 0; i < VECTORS.length; i++) {
      var k = VECTORS[i];
      if (cur[k] && !tgt[k]) return true;
    }
    return false;
  }

  function paintFp() {
    var lvl = FP_LABELS[fpState.preset] ? fpState.preset : "green";
    fpVal.textContent = FP_LABELS[lvl];
    var opts = fpMenu.querySelectorAll("li");
    for (var i = 0; i < opts.length; i++) {
      opts[i].setAttribute("aria-selected", opts[i].dataset.value === lvl ? "true" : "false");
    }
    var eff = fpEffective(fpState.preset, fpState.vectors);
    // Reduced = below the Green floor (Green itself is a first-class safe level).
    var reduced = !eff.on || isWeakening(GREEN_EFF, eff);
    fpLevelPill.textContent = lvl;
    // s2 = accent (red, strongest), s1 = brand (green/yellow/custom), s3 = warn
    // (off / below the Green floor) — the pill is a status display first.
    fpLevelPill.className = "tor-status s" + (lvl === "off" || reduced ? 3 : lvl === "red" ? 2 : 1);
    fpDetail.hidden = fpState.preset !== "custom";
    VECTORS.forEach(function (k) {
      var el = document.querySelector('.switch[data-fp="' + k + '"]');
      if (el) {
        el.classList.toggle("on", !!fpState.vectors[k]);
        el.setAttribute("aria-checked", fpState.vectors[k] ? "true" : "false");
      }
    });
  }

  function applyFp(level, vectors, confirm) {
    var req = { cmd: "set_hardening", level: level, confirm: !!confirm };
    if (vectors) req.vectors = vectors;
    return query(req);
  }

  // --- the gate (two confirmations in one dialog) ---
  var gate = document.getElementById("gate");
  var gateTitle = document.getElementById("gate-title");
  var gateBody = document.getElementById("gate-body");
  var gateCancel = document.getElementById("gate-cancel");
  var gateConfirm = document.getElementById("gate-confirm");
  var gatePending = null, gateStep = 1;

  function openGate(copy, onConfirm) {
    gatePending = onConfirm; gateStep = 1;
    gateTitle.textContent = copy.title;
    gateBody.innerHTML = copy.body;
    gateConfirm.textContent = "Weaken anyway";
    gate.hidden = false;
  }
  function closeGate() { gate.hidden = true; gatePending = null; gateStep = 1; }
  gateCancel.addEventListener("click", function () { closeGate(); });
  gateConfirm.addEventListener("click", function () {
    if (gateStep === 1) {
      gateStep = 2;
      gateBody.innerHTML = "This lowers <strong>your own</strong> protection. Continue anyway? " +
        "You can restore full protection here at any time.";
      gateConfirm.textContent = "Yes, weaken protection";
      return;
    }
    var fn = gatePending; closeGate(); if (fn) fn();
  });

  // Honest, plain-language cost of each step DOWN the Ampel ladder (CD-30). The
  // copy states exactly what disengages — never more, never less (rule 0.1).
  function presetGateCopy(level, from) {
    if (level === "off") {
      return {
        title: "Turn off tracking protection?",
        body: "With hardening <strong>off</strong>, every site gets a stable, distinctive " +
          "fingerprint — your canvas, GPU, audio and text measurements read the same across " +
          "sites and every session, so trackers can <strong>link your visits and recognise " +
          "you when you return</strong>, even without cookies. This makes you easier to track, not harder."
      };
    }
    if (level === "custom") {
      return {
        title: "Customise protection?",
        body: "Custom mode lets you disable individual protections. A partial, unusual set can make " +
          "you <strong>more</strong> identifiable, not less — an Ampel level (Green, Yellow or Red) " +
          "hides you better. Only turn things off if you have a specific reason."
      };
    }
    if (from === "red") {
      return {
        title: "Step down from Red?",
        body: "Red is maximum protection. Stepping down returns the noise and timer clamps from " +
          "their <strong>tightest</strong> setting to standard strength" +
          (level === "green" ? ", turns the clock-precision, media-codec and math-rounding clamps off" : "") +
          ", and unlocks the window size (free sizing returns). Sites see slightly more detail than they do now."
      };
    }
    return {
      title: "Step down to Green?",
      body: "Green keeps the coherent core (canvas, GPU, audio, text metrics, fonts, device profile, " +
        "screen) but turns the <strong>clock-precision, media-codec and math-rounding</strong> clamps " +
        "off. Sites regain those three measurements; everything else stays protected."
    };
  }

  function selectPreset(level) {
    if (level === fpState.preset && level !== "custom") return;
    var cur = fpEffective(fpState.preset, fpState.vectors);
    var tgt = fpEffective(level, fpState.vectors);
    var weaken = isWeakening(cur, tgt);
    var enteringCustom = level === "custom" && fpState.preset !== "custom";
    function commit() {
      var from = fpState.preset;
      fpState.preset = level;
      paintFp();
      applyFp(level, level === "custom" ? fpState.vectors : null, weaken)
        .catch(function (err) { fpState.preset = from; paintFp(); setStatus(String(err), true); });
    }
    if (weaken || enteringCustom) openGate(presetGateCopy(level, fpState.preset), commit);
    else commit();
  }

  function toggleVector(el) {
    var k = el.dataset.fp;
    var turningOff = !!fpState.vectors[k];
    function commit() {
      fpState.vectors[k] = !fpState.vectors[k];
      fpState.preset = "custom";
      paintFp();
      applyFp("custom", fpState.vectors, turningOff)
        .catch(function (err) { setStatus(String(err), true); });
    }
    if (turningOff) {
      openGate({
        title: "Disable " + VECTOR_LABELS[k] + " protection?",
        body: "Leaving <strong>" + VECTOR_LABELS[k] + "</strong> unprotected lets sites read its " +
          "real value and use it to recognise you across sites and sessions. Keep it protected " +
          "unless you have a specific reason."
      }, commit);
    } else commit();
  }

  function openFp(open) {
    fpSelect.classList.toggle("open", open);
    fpBtn.setAttribute("aria-expanded", open ? "true" : "false");
    fpMenu.hidden = !open;
  }
  fpBtn.addEventListener("click", function (e) {
    e.stopPropagation();
    openFp(!fpSelect.classList.contains("open"));
  });
  var fpOpts = fpMenu.querySelectorAll("li");
  for (var fi = 0; fi < fpOpts.length; fi++) {
    (function (li) {
      li.addEventListener("click", function (e) {
        e.stopPropagation();
        openFp(false);
        selectPreset(li.dataset.value);
      });
    })(fpOpts[fi]);
  }
  var fpSwitches = document.querySelectorAll(".switch[data-fp]");
  for (var fj = 0; fj < fpSwitches.length; fj++) {
    (function (el) {
      el.addEventListener("click", function () { toggleVector(el); });
      el.addEventListener("keydown", function (e) {
        if (e.key === " " || e.key === "Enter") { e.preventDefault(); toggleVector(el); }
      });
    })(fpSwitches[fj]);
  }

  // Click anywhere else closes the menus.
  document.addEventListener("click", function () { openEngine(false); openFp(false); openScreen(false); });

  // Load current values on startup.
  query({ cmd: "get_settings" })
    .then(function (response) {
      var s = JSON.parse(response);
      paint("feather_edges", s.feather_edges);
      paint("animated_background", s.animated_background);
      paint("stay_foreground", s.stay_foreground);
      paint("purge_residue", s.purge_residue);
      paint("tor_default", s.tor_default);
      paint("tor_enabled", s.tor_enabled);
      paint("rotate_on_restart", s.rotate_on_restart);
      paint("rotate_auto", s.rotate_auto);
      paint("rotate_new_circuit", s.rotate_new_circuit);
      paintGlow(s.glow_intensity);
      paintEngine(s.search_engine);
      paintScreen(s.screen_preset);
      paintRotInterval(s.rotate_interval_min);
      var lvl = canonLevel(s.fp_preset || "");
      fpState.preset = FP_LABELS[lvl] ? lvl : "green";
      if (s.fp_custom) {
        VECTORS.forEach(function (k) { fpState.vectors[k] = s.fp_custom[k] !== false; });
      }
      paintFp();
    })
    .catch(function (err) { setStatus(String(err), true); });

  // On-disk privacy readout (CD-34): the live browsing footprint + what the last
  // launch purge did. Truthful by construction — both numbers arrive measured from the
  // host, never asserted. Refreshed on load, after a toggle, and on a slow poll.
  var residuePill = document.getElementById("residue-pill");
  var residueLast = document.getElementById("residue-last");
  var residueNow = document.getElementById("residue-now");
  function refreshResidue() {
    return query({ cmd: "get_residue_footprint" }).then(function (r) {
      var j;
      try { j = JSON.parse(r); } catch (x) { return; }
      var lp = j.last_purge || {};
      var last;
      // lp.error is a self-describing sentence from the host (refused / could-not-resolve /
      // could-not-fully-clear), so it renders verbatim — no framing that would mislabel a
      // never-ran case as "incomplete" or double the phrasing.
      if (!lp.ran) last = "purge is off — residue accumulates on disk";
      else if (lp.error) last = lp.error;
      else if (lp.found_bytes > 0) last = "cleared " + lp.found_human + " of browsing residue";
      else last = "no residue found — clean";
      if (residueLast) residueLast.textContent = last;
      // The live profile is CEF's working scaffolding; it holds no browsing content
      // (that stays in RAM) and is wiped at the next launch. Say so, plainly.
      if (residueNow) {
        residueNow.textContent = j.on_disk_human +
          " — working profile, wiped next launch (holds no browsing content)";
      }
      if (residuePill) {
        residuePill.textContent = j.enabled ? "on" : "off";
        residuePill.className = "tor-status s" + (j.enabled ? 1 : 3);
      }
    }).catch(function () {});
  }
  refreshResidue();
  setInterval(refreshResidue, 3000);

  // Tor engine status readout (CD-15): polled while the settings page is open.
  // On failure the engine reports a concrete reason (timeout, bad dir, …) —
  // shown so "failed" is never a dead end (CD-15 HOTFIX Stage C).
  var torStatusEl = document.getElementById("tor-status");
  var torReasonEl = document.getElementById("tor-reason");
  var torVersionEl = document.getElementById("tor-version");
  var TOR_LABELS = ["off", "connecting…", "ready", "failed"];
  function pollTorStatus() {
    query({ cmd: "tor_status" }).then(function (r) {
      var st = 0, reason = "", version = "";
      try { var j = JSON.parse(r); st = j.status | 0; reason = j.reason || ""; version = j.version || ""; } catch (x) {}
      if (torStatusEl) {
        torStatusEl.textContent = TOR_LABELS[st] || "off";
        torStatusEl.className = "tor-status s" + st;
      }
      if (torReasonEl) {
        if (st === 3 && reason) {
          torReasonEl.textContent = reason;
          torReasonEl.hidden = false;
        } else {
          torReasonEl.textContent = "";
          torReasonEl.hidden = true;
        }
      }
      // The embedded arti (Tor engine) version — honest: this is the arti-client
      // crate we link, not the standalone Tor CLI (CD-18).
      if (torVersionEl && version) torVersionEl.textContent = "arti " + version;
    }).catch(function () {});
  }
  pollTorStatus();
  setInterval(pollTorStatus, 2000);

  // "New circuit / new identity" (CD-18): fresh Tor circuits for new requests.
  var newCircuitBtn = document.getElementById("tor-new-circuit");
  if (newCircuitBtn) {
    newCircuitBtn.addEventListener("click", function () {
      query({ cmd: "tor_new_circuit" }).then(function () {
        var was = newCircuitBtn.textContent;
        newCircuitBtn.textContent = "New circuit ✓";
        setTimeout(function () { newCircuitBtn.textContent = was; }, 1400);
      }).catch(function () {});
    });
  }

  // --- Vault (CD-40, D-0058; unlock model per CD-42, D-0062) ----------------
  // Config + lock controls (setup happens at first launch, on the lock page).
  // No secret ever enters this page: the host captures the master-password
  // keystrokes into locked memory and pushes only the masked character count
  // here (window.cdVault).
  (function () {
    var pill = document.getElementById("vault-pill");
    var lockRow = document.getElementById("vault-lock-row");
    var capture = document.getElementById("vault-capture");
    var capHint = document.getElementById("vault-cap-hint");
    var entry = document.getElementById("vault-entry");
    var dots = document.getElementById("vault-dots");
    var errEl = document.getElementById("vault-err");
    var lockBtn = document.getElementById("vault-lock");
    var cancelBtn = document.getElementById("vault-cancel");
    if (!pill) return;

    function renderDots(n) {
      var cap = Math.min(n, 64);
      while (dots.children.length > cap) dots.removeChild(dots.lastChild);
      while (dots.children.length < cap) {
        var d = document.createElement("span");
        d.className = "vdot";
        dots.appendChild(d);
      }
    }

    var HINTS = {
      change_pass: "New master password (at least 8 characters) — Enter to continue, Esc to cancel.",
      change_confirm: "Re-type the new master password to confirm — Enter to change it.",
      retune_kdf: "Enter your current master password to authorize the cost change — verified first, then re-derived under the new parameters."
    };
    var BUSY = {
      change_confirm: "Re-wrapping…",
      retune_kdf: "Verifying and re-deriving…"
    };

    var KINDS = { passphrase: "Master password", passkey: "Passkey" };
    var last = {};

    function render(s) {
      last = s;
      var label =
        s.vault === "none" ? "not set up" :
        s.vault === "unlocked" ? "unlocked" :
        s.vault === "bypassed" ? "DEV BYPASS" : "locked";
      pill.textContent = label;
      pill.className = "tor-status " + (s.vault === "unlocked" ? "s2" : s.vault === "bypassed" ? "s3" : "");

      // Any settings-side capture (change / re-tune) shows the entry.
      var capturing = s.capture === "change_pass" || s.capture === "change_confirm" ||
                      s.capture === "retune_kdf";
      lockRow.hidden = s.vault !== "unlocked" || capturing;
      capture.hidden = !capturing;
      if (capturing) {
        capHint.textContent = s.busy ? (BUSY[s.capture] || "Working…") : (HINTS[s.capture] || "");
        renderDots(s.chars || 0);
        entry.classList.toggle("busy", !!s.busy);
      }
      if (s.error && (capturing || s.vault === "unlocked")) {
        errEl.textContent = s.error;
        errEl.hidden = false;
      } else {
        errEl.hidden = true;
      }

      // The config surface (1c): methods, policy, KDF cost — unlocked only.
      var config = document.getElementById("vault-config");
      config.hidden = s.vault !== "unlocked" || capturing;
      if (!config.hidden) {
        var parts = [];
        var hasPasskey = (s.methods || []).some(function (m) { return m.kind === "passkey"; });
        for (var i = 0; i < (s.methods || []).length; i++) {
          var m = s.methods[i];
          parts.push((KINDS[m.kind] || m.kind) + (m.removable ? "" : " (always present)"));
        }
        document.getElementById("vault-methods").textContent =
          parts.length ? parts.join(" · ") : "—";
        var p1 = document.getElementById("vault-pol-1");
        var p2 = document.getElementById("vault-pol-2");
        p1.classList.toggle("active", s.required === 1);
        p2.classList.toggle("active", s.required === 2);
        // 2FA needs the passkey enrolled (the host refuses regardless — this
        // just keeps the button honest).
        p2.disabled = !hasPasskey;
        p2.title = hasPasskey ? "" : "Requires an enrolled passkey";
        if (s.kdf) {
          document.getElementById("vault-kdf-hint").textContent =
            Math.round(s.kdf.m_cost_kib / 1024) + " MiB · " + s.kdf.t_cost +
            (s.kdf.t_cost === 1 ? " pass" : " passes") + " · " + s.kdf.p_cost +
            (s.kdf.p_cost === 1 ? " lane" : " lanes");
        }
      }
    }

    window.cdVault = function (json) {
      try { render(JSON.parse(json)); } catch (e) {}
    };

    cancelBtn.addEventListener("click", function () {
      query({ cmd: "vault_cancel_capture" }).then(function (r) {
        try { render(JSON.parse(r)); } catch (e) {}
      }).catch(function () {});
    });

    // Locking ends the session (windows close by design — websites are not
    // saved). Two-step button instead of a modal: the second click confirms.
    var lockArmed = null;
    lockBtn.addEventListener("click", function () {
      if (lockArmed) {
        query({ cmd: "vault_lock" }).catch(function () {});
        return;
      }
      lockBtn.textContent = "Confirm lock";
      lockArmed = setTimeout(function () {
        lockBtn.textContent = "Lock";
        lockArmed = null;
      }, 3000);
    });

    // --- Config surface (1c) ------------------------------------------------
    function applyState(p) {
      p.then(function (r) {
        try { render(JSON.parse(r)); } catch (e) {}
      }).catch(function (e) {
        errEl.textContent = String(e);
        errEl.hidden = false;
      });
    }

    // Policy: turning 2FA on applies directly (strengthening); dropping back
    // to password-only is a weakening — two-step confirm, and the HOST
    // refuses without the flag.
    var polArmed = null;
    document.getElementById("vault-pol-2").addEventListener("click", function () {
      applyState(query({ cmd: "vault_set_policy", required: 2 }));
    });
    var pol1 = document.getElementById("vault-pol-1");
    pol1.addEventListener("click", function () {
      if (pol1.classList.contains("active")) return;
      if (polArmed) {
        clearTimeout(polArmed);
        polArmed = null;
        pol1.textContent = "Password only";
        applyState(query({ cmd: "vault_set_policy", required: 1, confirm: true }));
        return;
      }
      pol1.textContent = "Confirm password-only";
      polArmed = setTimeout(function () {
        pol1.textContent = "Password only";
        polArmed = null;
      }, 3000);
    });

    // KDF re-tune: numbers staged here, authorized by a host-captured
    // passphrase entry. Lowering below the default needs the confirm flag —
    // armed the same two-step way.
    var retuneForm = document.getElementById("vault-retune-form");
    document.getElementById("vault-retune").addEventListener("click", function () {
      retuneForm.hidden = !retuneForm.hidden;
      if (!retuneForm.hidden && last.kdf) {
        document.getElementById("kdf-m").value = last.kdf.m_cost_kib;
        document.getElementById("kdf-t").value = last.kdf.t_cost;
        document.getElementById("kdf-p").value = last.kdf.p_cost;
      }
    });
    var kdfArmed = false;
    var kdfApply = document.getElementById("kdf-apply");
    kdfApply.addEventListener("click", function () {
      var m = parseInt(document.getElementById("kdf-m").value, 10) || 0;
      var t = parseInt(document.getElementById("kdf-t").value, 10) || 0;
      var p = parseInt(document.getElementById("kdf-p").value, 10) || 0;
      var req = { cmd: "vault_retune_kdf", m_cost_kib: m, t_cost: t, p_cost: p };
      if (kdfArmed) { req.confirm = true; }
      query(req).then(function (r) {
        kdfArmed = false;
        kdfApply.textContent = "Apply…";
        retuneForm.hidden = true;
        try { render(JSON.parse(r)); } catch (e) {}
      }).catch(function (e) {
        var msg = String(e);
        if (msg.indexOf("confirmation") !== -1 && !kdfArmed) {
          kdfArmed = true;
          kdfApply.textContent = "Confirm weaker cost";
          setTimeout(function () {
            kdfArmed = false;
            kdfApply.textContent = "Apply…";
          }, 4000);
        }
        errEl.textContent = msg;
        errEl.hidden = false;
      });
    });

    document.getElementById("vault-change").addEventListener("click", function () {
      applyState(query({ cmd: "vault_begin_capture", purpose: "change_pass" }));
    });

    query({ cmd: "get_vault_state" }).then(function (r) {
      try { render(JSON.parse(r)); } catch (e) {}
    }).catch(function () {});
  })();
})();
