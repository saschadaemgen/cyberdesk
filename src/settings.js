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

  var switches = document.querySelectorAll(".switch");
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
  var ENGINE_LABELS =
    { google: "Google", duckduckgo: "DuckDuckGo", bing: "Bing", startpage: "Startpage" };

  function paintEngine(value) {
    var v = ENGINE_LABELS[value] ? value : "google";
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

  // Click anywhere else closes the menu.
  document.addEventListener("click", function () { openEngine(false); });

  // Load current values on startup.
  query({ cmd: "get_settings" })
    .then(function (response) {
      var s = JSON.parse(response);
      paint("feather_edges", s.feather_edges);
      paint("animated_background", s.animated_background);
      paint("stay_foreground", s.stay_foreground);
      paint("tor_default", s.tor_default);
      paint("tor_enabled", s.tor_enabled);
      paintGlow(s.glow_intensity);
      paintEngine(s.search_engine);
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
