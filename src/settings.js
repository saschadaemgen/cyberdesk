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
    paint(key, next);
    setStatus("");
    query({ cmd: "set_setting", key: key, value: next })
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
  var VECTORS = ["canvas", "webgl", "gpu", "audio", "metrics", "nav", "fonts", "timing", "media", "math"];
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
})();
