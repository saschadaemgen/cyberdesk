// Floating HUD strip (CD-30 Task B). Talks to the Rust host over the CEF message
// router (window.cefQuery) only - no network, no fetch, no external resources.
// The host pushes state on change via window.cdHud(json) (like cdFrame); this
// page pulls once on load (get_hud_state) and only ticks the CLOCK and the
// rotation COUNTDOWN locally between pushes. Every displayed value is real
// (CD-30 rule 0.1) - the countdown/age anchors are absolute timestamps computed
// from the push, so a stale cache can never show a wrong deadline. Wire format
// in docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) { reject("hud IPC unavailable"); return; }
      window.cefQuery({
        request: JSON.stringify(req),
        persistent: false,
        onSuccess: function (r) { resolve(r); },
        onFailure: function (code, msg) { reject(msg || ("error " + code)); }
      });
    });
  }

  var clockEl = document.getElementById("clock");
  var levelField = document.getElementById("f-level");
  var levelV = document.getElementById("level-v");
  var vectorsV = document.getElementById("vectors-v");
  var routeK = document.getElementById("route-k");
  var routeV = document.getElementById("route-v");
  var identityK = document.getElementById("identity-k");
  var identityV = document.getElementById("identity-v");
  var ampelEl = document.getElementById("ampel");
  var fieldsEl = document.getElementById("fields");
  var menuEl = document.getElementById("menu");
  var gateEl = document.getElementById("gate");
  var gateText = document.getElementById("gate-text");
  var gateKeep = document.getElementById("gate-keep");
  var gateWeaken = document.getElementById("gate-weaken");

  var state = null;      // last pushed payload (parsed)
  var deadline = null;   // absolute ms (unix) of the next automatic rotation
  var bornAbs = null;    // absolute ms (unix) the current identity was minted

  function two(n) { return n < 10 ? "0" + n : "" + n; }

  // Re-anchor the countdown / age to ABSOLUTE times at receive time: the payload
  // carries elapsed-based fields stamped with the host's send time, so the page
  // never accumulates drift and a re-pulled cache stays correct.
  function anchor() {
    var sent = state && typeof state.sent_ms === "number" ? state.sent_ms : Date.now();
    var r = state && state.rotate;
    deadline = r && r.auto
      ? sent + Math.max(0, r.interval_min * 60000 - (r.elapsed_ms || 0))
      : null;
    bornAbs = sent - ((state && state.identity_age_ms) || 0);
  }

  // Sascha's digital clock: local wall-clock time. The PROCESS runs under TZ=UTC
  // (the CD-16 timezone clamp, honest and global), so local time is derived from
  // the host-supplied UTC offset - never from getHours(), which would silently
  // show UTC mislabeled as local.
  function paintClock() {
    if (!state || typeof state.tz_offset_min !== "number") { clockEl.textContent = "--:--:--"; return; }
    var d = new Date(Date.now() + state.tz_offset_min * 60000);
    clockEl.textContent = two(d.getUTCHours()) + ":" + two(d.getUTCMinutes()) + ":" + two(d.getUTCSeconds());
  }

  function fmtCountdown(ms) {
    var s = Math.max(0, Math.round(ms / 1000));
    var h = Math.floor(s / 3600);
    var m = Math.floor((s % 3600) / 60);
    var sec = s % 60;
    return h > 0 ? h + ":" + two(m) + ":" + two(sec) : m + ":" + two(sec);
  }

  function fmtAge(ms) {
    var s = Math.max(0, Math.floor(ms / 1000));
    if (s < 60) return s + " s";
    var m = Math.floor(s / 60);
    if (m < 60) return m + " min";
    var h = Math.floor(m / 60);
    if (h < 48) return h + " h " + two(m % 60) + " min";
    return Math.floor(h / 24) + " d";
  }

  // The identity field ticks locally between pushes (countdown / age).
  function paintIdentity() {
    if (!state) { identityV.textContent = "-"; return; }
    if (deadline != null) {
      identityK.textContent = "New identity in";
      identityV.textContent = fmtCountdown(deadline - Date.now());
    } else {
      identityK.textContent = "Identity age";
      identityV.textContent = bornAbs != null ? fmtAge(Date.now() - bornAbs) : "-";
    }
  }

  var prevLevel = null;
  function paint() {
    if (!state) return;
    // Protection level - the Ampel state in text, with the honest tint rules:
    // warn when genuinely reduced/off, accent at Red (the strongest).
    var level = (state.level || "").toUpperCase() || "-";
    levelV.textContent = level;
    levelField.classList.toggle("warn", !!state.reduced || state.level === "off");
    levelField.classList.toggle("good", state.level === "red" && !state.reduced);
    // The Ampel lamps - lit strictly from the ACTIVE global level (rule 0.1).
    // Entering Red slams the lamp (the page-side echo of the grid transition -
    // it can only fire on a real push carrying the committed Red level).
    ampelEl.setAttribute("data-level", state.level || "");
    if (state.level === "red" && prevLevel !== null && prevLevel !== "red") {
      ampelEl.classList.remove("slam");
      void ampelEl.offsetWidth; // restart the one-shot animation
      ampelEl.classList.add("slam");
    }
    prevLevel = state.level;
    var lvs = menuEl.querySelectorAll(".lv");
    for (var i = 0; i < lvs.length; i++) {
      lvs[i].classList.toggle("active", lvs[i].getAttribute("data-level") === state.level);
    }
    // Honest live vector count (N/N) - the global effective config.
    var on = state.vectors_on | 0;
    var total = state.vectors_total | 0;
    vectorsV.textContent = total ? on + "/" + total + " active" : "-";
    // The ACTIVE window's route (CD-15 state, surfaced as text). "Onion" is
    // shown only while the window is actually on an onion service (CD-35 Task
    // C): the host derives it from the live URL + Tor mode - connected to an
    // onion service, resolved inside Tor, nothing more claimed.
    var r = state.route || {};
    routeK.textContent = "Route W" + (r.window || 1);
    routeV.textContent = r.tor ? (r.onion ? "Tor · Onion" : "Tor") : "Clearnet";
    routeV.parentElement.classList.toggle("on", !!r.tor);
    // Vault status (CD-40 1c). While the HUD exists the gate is open, so the
    // honest states are: unlocked (sealed state active), not set up, or the
    // debug-build dev bypass (loudly warned - the gate was skipped).
    var vaultV = document.getElementById("vault-v");
    if (vaultV) {
      var vs = state.vault || "none";
      vaultV.textContent = vs === "unlocked" ? "Unlocked" : vs === "bypassed" ? "DEV BYPASS" : "Not set up";
      vaultV.parentElement.classList.toggle("on", vs === "unlocked");
      vaultV.parentElement.classList.toggle("warn", vs === "bypassed");
    }
    paintIdentity();
    paintClock();
  }

  // --- The global Ampel control (CD-30 Task C) -----------------------------
  // Click → the level menu (replaces the fields row). Stepping UP the ladder
  // (Off < Green < Yellow < Red) applies immediately; stepping DOWN opens the
  // inline two-confirmation gate with the honest cost. The host re-validates
  // every weakening (set_hardening confirm), so this gate is UX, not the trust
  // boundary. "Custom…" opens the settings card (the per-vector view lives there).
  var RANK = { off: 0, green: 1, yellow: 2, red: 3 };
  var gatePending = null, gateStep = 1;

  function showMenu(open) {
    menuEl.hidden = !open;
    fieldsEl.hidden = open;
    gateEl.hidden = true;
    gatePending = null;
    ampelEl.setAttribute("aria-expanded", open ? "true" : "false");
  }

  // Honest plain-language cost of the step down (mirrors the settings gate copy).
  function gateCopy(target) {
    var from = state && state.level;
    if (target === "off") {
      return "Turn protection OFF? Every site then gets a stable, distinctive fingerprint " +
        "and can recognise you when you return - this makes you easier to track, not harder.";
    }
    if (from === "red") {
      return "Step down from Red (maximum)? The noise and timer clamps return from their " +
        "tightest setting to standard strength" +
        (target === "green" ? ", the clock, media-codec and math clamps turn off" : "") +
        ", and the window size unlocks.";
    }
    return "Step down to Green? The clock-precision, media-codec and math-rounding clamps " +
      "turn off; the coherent core stays protected.";
  }

  function applyLevel(level, confirm) {
    return query({ cmd: "set_hardening", level: level, confirm: !!confirm });
  }

  function openGate(target) {
    gatePending = target;
    gateStep = 1;
    gateText.textContent = gateCopy(target);
    gateWeaken.textContent = "Weaken anyway";
    menuEl.hidden = true;
    fieldsEl.hidden = true;
    gateEl.hidden = false;
  }

  function chooseLevel(target) {
    if (target === "custom") {
      // The per-vector Custom view lives in the settings card underneath.
      query({ cmd: "open_settings" }).catch(function () {});
      showMenu(false);
      return;
    }
    var cur = state && RANK[state.level] != null ? RANK[state.level] : null;
    var tgt = RANK[target];
    var weaken = cur != null && tgt < cur;
    if (weaken) { openGate(target); return; }
    showMenu(false);
    // Up / sideways: immediate. If the host still classifies it as a weakening
    // (e.g. coming from Custom), it rejects - then run the gate honestly.
    applyLevel(target, false).catch(function () { openGate(target); });
  }

  ampelEl.addEventListener("click", function (ev) {
    ev.stopPropagation();
    showMenu(menuEl.hidden);
  });
  Array.prototype.forEach.call(menuEl.querySelectorAll(".lv"), function (btn) {
    btn.addEventListener("click", function (ev) {
      ev.stopPropagation();
      chooseLevel(btn.getAttribute("data-level"));
    });
  });
  gateKeep.addEventListener("click", function (ev) {
    ev.stopPropagation();
    showMenu(false);
  });
  gateWeaken.addEventListener("click", function (ev) {
    ev.stopPropagation();
    if (gateStep === 1) {
      gateStep = 2;
      gateText.textContent = "Lower your own protection? You can restore it here at any time.";
      gateWeaken.textContent = "Yes, weaken";
      return;
    }
    var target = gatePending;
    showMenu(false);
    if (target) { applyLevel(target, true).catch(function () {}); }
  });
  document.addEventListener("click", function () { if (!menuEl.hidden || !gateEl.hidden) showMenu(false); });

  // Host push entry point (on change, never per frame).
  window.cdHud = function (json) {
    try { state = JSON.parse(json); } catch (e) { return; }
    anchor();
    paint();
  };

  // Local ticker: clock + countdown/age only (all other fields are push-driven).
  setInterval(function () { paintClock(); paintIdentity(); }, 500);

  // Pull once on load (the host may have pushed before this page existed).
  query({ cmd: "get_hud_state" }).then(function (json) {
    if (!state) { window.cdHud(json); }
  }).catch(function () {});
})();
