// Floating command sets (CD-12, D-0021). The CD-08 single bar is gone: this page
// draws N per-window ensembles on a TRANSPARENT body plus one shared favorites
// launcher. The HOST positions each ensemble above its column and tells the page
// which slot is engaged (hovered) via window.cdFrame(json); the page reveals that
// ensemble (CSS fade+drop, ~220 ms) and binds its capsule / orbs / star / star to
// THAT column. All logic (suggestions, star, scheme, nav) is reused from CD-07/08
// per capsule; only the presentation and per-slot binding changed. Talks to the
// host over the CEF message router only (window.cefQuery) - no network.
// Wire format: see docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) { reject("command IPC unavailable"); return; }
      window.cefQuery({
        request: JSON.stringify(req),
        persistent: false,
        onSuccess: function (r) { resolve(r); },
        onFailure: function (c, m) { reject(m || ("error " + c)); }
      });
    });
  }

  var STAR_PATH =
    "M12 3.6l2.6 5.27 5.82.85-4.21 4.1.99 5.79L12 16.87l-5.2 2.74.99-5.79-4.21-4.1 5.82-.85z";
  var SVG = {
    back: '<svg viewBox="0 0 24 24" width="16" height="16"><path d="M15 5l-7 7 7 7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    fwd: '<svg viewBox="0 0 24 24" width="16" height="16"><path d="M9 5l7 7-7 7" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    reload: '<svg viewBox="0 0 24 24" width="15" height="15"><path d="M20 11a8 8 0 10-.9 5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/><path d="M20 4v5h-5" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
    lock: '<svg viewBox="0 0 24 24" width="14" height="14"><rect x="5" y="11" width="14" height="9" rx="1.5" fill="currentColor"/><path d="M8 11V8a4 4 0 018 0v3" fill="none" stroke="currentColor" stroke-width="2"/></svg>',
    star: '<svg viewBox="0 0 24 24" width="17" height="17"><path class="star-path" d="' + STAR_PATH + '" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/></svg>',
    // A shield (privacy motif) for the per-window Tor toggle (CD-15).
    tor: '<svg viewBox="0 0 24 24" width="16" height="16"><path d="M12 3l7 3v5c0 4.6-3 7.6-7 9-4-1.4-7-4.4-7-9V6l7-3z" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round"/></svg>',
    // A fingerprint (tracking-resistance motif) for the per-window hardening control (CD-25).
    fp: '<svg viewBox="0 0 24 24" width="16" height="16"><path d="M12 5c-3.3 0-6 2.7-6 6v3M18 12v-1c0-3.3-2.7-6-6-6M9 12a3 3 0 016 0v3M12 12v6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round"/></svg>',
    // An X for the per-window close icon (CD-18).
    close: '<svg viewBox="0 0 24 24" width="15" height="15"><path d="M6 6l12 12M18 6L6 18" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"/></svg>'
  };

  var band = document.getElementById("band");
  var launcher = document.getElementById("launcher");
  var ensembles = {};   // slot id -> ensemble state
  var engaged = null;   // engaged (hovered) slot id, or null

  // --- One per-window ensemble --------------------------------------------

  function makeEnsemble(id) {
    var el = document.createElement("div");
    el.className = "ensemble";
    // CD-18: the anonymity (Tor) icon and the close icon are two ALWAYS-PRESENT
    // controls sitting immediately to the RIGHT of the address capsule (the capsule
    // is the sole flex-grow child, so they pin to its right edge). They drive THIS
    // window only. This consolidates the CD-15 Tor glyph (moved here from the left
    // of the capsule) and the retired CD-12 corner-hover close orb.
    el.innerHTML =
      '<div class="ens-row">' +
        '<button class="orb" data-act="go_back" title="Back" aria-label="Back">' + SVG.back + '</button>' +
        '<button class="orb" data-act="go_forward" title="Forward" aria-label="Forward">' + SVG.fwd + '</button>' +
        '<button class="orb" data-act="reload" title="Reload" aria-label="Reload">' + SVG.reload + '</button>' +
        '<div class="capsule">' +
          '<span class="scheme neutral" aria-hidden="true">' + SVG.lock + '</span>' +
          '<input class="url" type="text" spellcheck="false" autocomplete="off" autocapitalize="off" placeholder="Search or enter address">' +
          '<button class="star" title="Favorite (Ctrl+D)" aria-label="Favorite" aria-pressed="false">' + SVG.star + '</button>' +
        '</div>' +
        '<button class="fp-orb" title="Protection level for this window" aria-label="Protection level for this window" aria-haspopup="menu">' +
          '<span class="lamp lamp-r"></span><span class="lamp lamp-y"></span><span class="lamp lamp-g"></span>' +
        '</button>' +
        '<button class="tor-orb" title="Route this window through Tor" aria-label="Toggle Tor for this window" aria-pressed="false">' + SVG.tor + '</button>' +
        '<button class="close-btn" title="Close this window (Ctrl+W)" aria-label="Close this window">' + SVG.close + '</button>' +
      '</div>' +
      '<div class="suggestions" role="listbox" aria-label="Suggestions"></div>';
    band.appendChild(el);

    var e = {
      id: id, el: el,
      input: el.querySelector(".url"),
      scheme: el.querySelector(".scheme"),
      star: el.querySelector(".star"),
      fp: el.querySelector(".fp-orb"),
      tor: el.querySelector(".tor-orb"),
      close: el.querySelector(".close-btn"),
      capsule: el.querySelector(".capsule"),
      list: el.querySelector(".suggestions"),
      currentUrl: "", currentTitle: "", pristine: "",
      suggestions: [], sel: -1, debounce: null, lastTyping: false
    };
    wire(e);
    ensembles[id] = e;
    return e;
  }

  function applyScheme(e, s) {
    e.scheme.className = "scheme " + (s === "https" ? "secure" : (s === "http" ? "insecure" : "neutral"));
  }
  function paintStar(e, fav) {
    e.star.classList.toggle("on", !!fav);
    e.star.setAttribute("aria-pressed", fav ? "true" : "false");
  }

  // Paint the per-window mini-Ampel (CD-25 orb → CD-30 graded traffic light)
  // from the frame-state fields. The lamp matching the EFFECTIVE level lights
  // (green/yellow/red); Off leaves all lamps dark; Custom reads as a distinct
  // brand tint. Off / reduced is warn-tinted (honesty: a reduced state must LOOK
  // reduced); a per-window override carries a small marker.
  var FP_LEVEL_NAMES = ["off", "green", "yellow", "red", "custom"];
  function paintFpOrb(e, d) {
    if (!e.fp) return;
    var lvl = FP_LEVEL_NAMES[d.fp] || "green";
    e.fp.setAttribute("data-level", lvl);
    e.fp.classList.toggle("reduced", d.reduced || d.fp === 0);
    e.fp.classList.toggle("override", !d.inherited);
    var shown = d.fp === 0 ? "off" : d.reduced ? "reduced (" + lvl + ")" : lvl;
    e.fp.title = "Protection level: " + shown +
      (d.inherited ? " (global default)" : " (window override)");
  }

  // Load this ensemble's own column nav state (scheme, star, prefill).
  function loadState(e, autofocus) {
    query({ cmd: "get_nav_state", slot: e.id }).then(function (r) {
      var s = JSON.parse(r);
      e.currentUrl = s.url || "";
      e.currentTitle = s.title || "";
      applyScheme(e, s.scheme);
      paintStar(e, s.favorite);
      e.pristine = s.url || "";
      e.input.value = e.pristine;
      if (autofocus) { e.input.focus(); e.input.select(); }
      suggest(e);
    }).catch(function () { suggest(e); });
  }

  function suggestText(e) { return e.input.value === e.pristine ? "" : e.input.value; }

  // The ensemble's suggestion list (typed query). Empty query -> cleared list
  // (the favorites live in the shared launcher now, not per capsule).
  function suggest(e) {
    var q = suggestText(e);
    if (q === "") { renderSuggest(e, []); return; }
    query({ cmd: "query_suggestions", input: q }).then(function (r) {
      var items; try { items = JSON.parse(r); } catch (x) { items = []; }
      renderSuggest(e, items);
    }).catch(function () { renderSuggest(e, []); });
  }
  function scheduleSuggest(e) {
    if (e.debounce) clearTimeout(e.debounce);
    e.debounce = setTimeout(function () { suggest(e); }, 90);
  }

  function renderSuggest(e, items) {
    e.suggestions = items || [];
    e.sel = -1;
    e.list.textContent = "";
    for (var i = 0; i < e.suggestions.length; i++) {
      var s = e.suggestions[i];
      var row = document.createElement("div");
      row.className = "suggest";
      row.setAttribute("role", "option");
      if (s.favorite) {
        row.insertAdjacentHTML("beforeend",
          '<svg class="row-star" viewBox="0 0 24 24"><path d="' + STAR_PATH + '" fill="currentColor"/></svg>');
      }
      var hasTitle = !!(s.title && s.title.length);
      var t = document.createElement("span");
      t.className = "row-title";
      t.textContent = hasTitle ? s.title : s.url;
      row.appendChild(t);
      if (hasTitle) {
        var u = document.createElement("span");
        u.className = "row-url";
        u.textContent = s.url;
        row.appendChild(u);
      }
      (function (url) {
        row.addEventListener("mousedown", function (ev) { ev.preventDefault(); });
        row.addEventListener("click", function () { navigate(e, url); });
      })(s.url);
      e.list.appendChild(row);
    }
  }
  function updateSel(e) {
    var rows = e.list.children;
    for (var i = 0; i < rows.length; i++) rows[i].classList.toggle("sel", i === e.sel);
    if (e.sel >= 0 && rows[e.sel]) rows[e.sel].scrollIntoView({ block: "nearest" });
  }

  function navigate(e, value) { query({ cmd: "navigate", slot: e.id, input: value }); }

  // Only the engaged ensemble reports typing (the host tracks one engaged slot).
  function reportTyping(e) {
    var active = (engaged === e.id) && document.activeElement === e.input && e.input.value.length > 0;
    if (active !== e.lastTyping) {
      e.lastTyping = active;
      query({ cmd: "bar_typing", active: active }).catch(function () {});
    }
  }

  function toggleFav(e) {
    if (!e.currentUrl) return;
    query({ cmd: "toggle_favorite", url: e.currentUrl, title: e.currentTitle }).then(function (r) {
      try { paintStar(e, JSON.parse(r).favorite); } catch (x) {}
      loadLauncher();
    }).catch(function () {});
  }

  function wire(e) {
    e.input.addEventListener("input", function () { scheduleSuggest(e); reportTyping(e); });
    e.input.addEventListener("focus", function () { e.capsule.classList.add("focus"); reportTyping(e); });
    e.input.addEventListener("blur", function () { e.capsule.classList.remove("focus"); reportTyping(e); });
    e.input.addEventListener("keydown", function (ev) {
      if (ev.key === "ArrowDown") {
        ev.preventDefault();
        if (e.suggestions.length) { e.sel = e.sel + 1; if (e.sel >= e.suggestions.length) e.sel = -1; updateSel(e); }
      } else if (ev.key === "ArrowUp") {
        ev.preventDefault();
        if (e.suggestions.length) { e.sel = e.sel - 1; if (e.sel < -1) e.sel = e.suggestions.length - 1; updateSel(e); }
      } else if (ev.key === "Enter") {
        ev.preventDefault();
        if (e.sel >= 0 && e.suggestions[e.sel]) navigate(e, e.suggestions[e.sel].url);
        else navigate(e, e.input.value);
      }
    });
    e.star.addEventListener("click", function () { toggleFav(e); });
    // Per-window tracking-resistance menu (CD-25): opens the level chooser for THIS
    // column. Weakening is gated inside the menu; strengthening applies at once.
    e.fp.addEventListener("click", function (ev) { ev.stopPropagation(); openFpMenu(e); });
    // Per-window Tor toggle (CD-15): flips THIS column between clearnet and Tor;
    // the host respawns its browser under the new context. The lit/connecting
    // state is set from the frame push (cdFrame), not optimistically.
    e.tor.addEventListener("click", function () {
      query({ cmd: "toggle_tor", slot: e.id }).catch(function () {});
    });
    // Per-window close (CD-18): closes THIS column. The host enforces
    // last-slot-refuses, so the final window can't be closed away.
    e.close.addEventListener("click", function () {
      query({ cmd: "close_slot", slot: e.id }).catch(function () {});
    });
    var orbs = e.el.querySelectorAll(".orb");
    for (var i = 0; i < orbs.length; i++) {
      (function (b) {
        b.addEventListener("click", function () {
          query({ cmd: b.dataset.act, slot: e.id }).then(function () { loadState(e, false); }).catch(function () {});
        });
      })(orbs[i]);
    }
  }

  // Ctrl+D toggles the engaged column's favorite (mirrors the surf-view Ctrl+D).
  document.addEventListener("keydown", function (ev) {
    if ((ev.ctrlKey || ev.metaKey) && (ev.key === "d" || ev.key === "D")) {
      ev.preventDefault();
      if (engaged != null && ensembles[engaged]) toggleFav(ensembles[engaged]);
    }
  });

  // --- Shared favorites launcher (tiles) ----------------------------------

  function loadLauncher() {
    query({ cmd: "query_suggestions", input: "" }).then(function (r) {
      var items; try { items = JSON.parse(r); } catch (x) { items = []; }
      launcher.textContent = "";
      for (var i = 0; i < items.length; i++) {
        var f = items[i];
        var tile = document.createElement("button");
        tile.className = "tile";
        tile.title = (f.title && f.title.length) ? f.title : f.url;
        tile.textContent = initial(f.title, f.url);
        tile.dataset.url = f.url;
        tile.dataset.ttl = f.title || "";
        wireTile(tile);
        launcher.appendChild(tile);
      }
    }).catch(function () {});
  }
  function initial(title, url) {
    var s = (title && title.trim()) || url.replace(/^https?:\/\/(www\.)?/, "");
    var c = (s || "?").trim().charAt(0);
    return c ? c.toUpperCase() : "?";
  }
  // Click navigates the engaged column; a mousedown + movement past a small
  // threshold starts a host-owned drag (CD-12): the page fires drag_start and the
  // shell draws the ghost + gutter drop zones from there on.
  function wireTile(tile) {
    var down = false, fired = false, sx = 0, sy = 0;
    tile.addEventListener("mousedown", function (ev) {
      ev.preventDefault();
      down = true; fired = false; sx = ev.clientX; sy = ev.clientY;
    });
    tile.addEventListener("mousemove", function (ev) {
      if (!down || fired) return;
      var dx = ev.clientX - sx, dy = ev.clientY - sy;
      if (dx * dx + dy * dy > 36) {      // ~6 px threshold
        fired = true; down = false;
        tile.classList.add("dragging");
        query({ cmd: "drag_start", url: tile.dataset.url, title: tile.dataset.ttl }).catch(function () {});
      }
    });
    tile.addEventListener("mouseup", function () {
      if (down && !fired && engaged != null && ensembles[engaged]) {
        navigate(ensembles[engaged], tile.dataset.url); // a click, not a drag
      }
      down = false;
    });
  }

  // --- Per-window tracking-resistance menu (CD-25) ------------------------
  // A small floating menu opened from each ensemble's .fp-orb: Use global default /
  // Standard / Strict / Off. Weakening (Off, or below the current effective level)
  // opens an inline TWO-step confirmation before it applies; strengthening applies
  // at once. The host re-validates weakening, so a choice the client can't classify
  // (Inherit that resolves weaker) is caught by the host and gated on the rejection.
  var fpPop = document.createElement("div");
  fpPop.className = "fp-pop";
  fpPop.hidden = true;
  fpPop.addEventListener("click", function (ev) { ev.stopPropagation(); });
  document.body.appendChild(fpPop);

  var FP_OPTS = [
    { level: "inherit", label: "Use global default" },
    { level: "green", label: "Green - everyday" },
    { level: "yellow", label: "Yellow - elevated" },
    { level: "red", label: "Red - maximum, locks size" },
    { level: "custom", label: "Custom…" },
    { level: "off", label: "Off" }
  ];
  // The full per-vector list (CD-29 Task C): every vector settable per-window, not
  // just presets. Canonical order matches harden.rs::VECTOR_KEYS.
  var FP_VECTORS = ["canvas", "webgl", "gpu", "audio", "metrics", "nav", "fonts", "timing", "media", "math"];
  var FP_VECTOR_LABELS = {
    canvas: "Canvas", webgl: "WebGL readback", gpu: "GPU identity", audio: "Audio",
    metrics: "Layout & text metrics", nav: "Device profile", fonts: "Fonts",
    timing: "Clock precision", media: "Media & codecs", math: "Math rounding"
  };
  function closeFpPop() { fpPop.hidden = true; }

  function applySlotFp(id, level, confirm, onNeedGate) {
    query({ cmd: "set_slot_hardening", slot: id, level: level, confirm: !!confirm })
      .catch(function () {
        // Host rejected an unconfirmed weakening the client didn't flag (e.g. an
        // Inherit that resolves weaker) - show the gate, then retry confirmed.
        if (!confirm && onNeedGate) onNeedGate();
      });
  }

  function chooseLevel(e, level) {
    // "Custom…" opens the per-vector detail (fetched from the host), not a preset.
    if (level === "custom") { openFpCustom(e); return; }
    // CD-30: the Ampel ladder Off < Green < Yellow < Red is a strict protection
    // order - any step DOWN it is a weakening and opens the two-step gate. The
    // frame's `fp` code IS the ladder rank for 0..3 (4 = custom, rank unknown).
    // Anything the client can't rank (from Custom, or an Inherit that resolves
    // weaker) is caught by the authoritative host classifier and gated on the
    // rejection (see applySlotFp).
    var RANK = { off: 0, green: 1, yellow: 2, red: 3 };
    var d = e.fpData || { fp: 1 };
    var curRank = d.fp >= 0 && d.fp <= 3 ? d.fp : null;
    var tgtRank = RANK[level] != null ? RANK[level] : null;
    var weaken = curRank != null && tgtRank != null && tgtRank < curRank;
    function commitConfirmed() { applySlotFp(e.id, level, true, null); }
    if (weaken) {
      fpGate(level, commitConfirmed);
    } else {
      closeFpPop();
      applySlotFp(e.id, level, false, function () { fpGate(level, commitConfirmed); });
    }
  }

  // --- Per-window per-vector Custom detail (CD-29 Task C) ------------------
  // Fetch the slot's current effective config from the host, then show a switch per
  // vector. Turning a vector OFF is a weakening: it opens the same two-step gate and
  // applies confirmed. Turning one ON applies at once. Each change sends the FULL
  // vectors object under level "custom", so the window becomes a per-vector override.
  function openFpCustom(e) {
    query({ cmd: "get_slot_hardening", slot: e.id }).then(function (r) {
      var d; try { d = JSON.parse(r); } catch (x) { return; }
      var cfg = (d && d.config) || {};
      var vectors = {};
      FP_VECTORS.forEach(function (k) { vectors[k] = cfg[k] !== false; });
      renderFpCustom(e, vectors);
    }).catch(function () {});
  }

  function applySlotCustom(e, vectors, confirm, onNeedGate) {
    query({ cmd: "set_slot_hardening", slot: e.id, level: "custom", vectors: vectors, confirm: !!confirm })
      .catch(function () { if (!confirm && onNeedGate) onNeedGate(); });
  }

  function renderFpCustom(e, vectors) {
    fpPop.innerHTML = "";
    var head = document.createElement("div");
    head.className = "fp-pop-title";
    head.textContent = "Custom · this window";
    fpPop.appendChild(head);
    var sub = document.createElement("div");
    sub.className = "fp-pop-sub";
    sub.textContent = "Turn individual protections on or off. A partial set can make this window easier to fingerprint.";
    fpPop.appendChild(sub);

    FP_VECTORS.forEach(function (k) {
      var row = document.createElement("button");
      row.className = "fp-pop-opt fp-vec" + (vectors[k] ? " active" : "");
      row.type = "button";
      row.textContent = (vectors[k] ? "● " : "○ ") + FP_VECTOR_LABELS[k];
      row.addEventListener("click", function (ev) {
        ev.stopPropagation();
        var turningOff = !!vectors[k];
        function commit() {
          vectors[k] = !vectors[k];
          renderFpCustom(e, vectors); // re-render in place (popup stays open)
          applySlotCustom(e, vectors, turningOff, null);
        }
        if (turningOff) {
          // Two-step gate, then commit confirmed - mirrors the preset weakening gate.
          fpGate("vector", function () { commit(); });
        } else {
          commit();
        }
      });
      fpPop.appendChild(row);
    });

    var back = document.createElement("button");
    back.className = "fp-pop-opt fp-pop-back";
    back.type = "button";
    back.textContent = "‹ Back to levels";
    back.addEventListener("click", function (ev) { ev.stopPropagation(); openFpMenu(e); });
    fpPop.appendChild(back);
    fpPop.hidden = false;
  }

  function fpGate(level, onConfirm) {
    fpPop.innerHTML = "";
    var warn = document.createElement("div");
    warn.className = "fp-pop-warn";
    warn.innerHTML = level === "off"
      ? "Turning protection <b>off</b> gives every site a stable fingerprint - trackers can link your visits and recognise you when you return. Weaken this window anyway?"
      : "This lowers this window's protection, making it easier to fingerprint. Weaken anyway?";
    fpPop.appendChild(warn);
    var step = 1;
    var actions = document.createElement("div");
    actions.className = "fp-pop-actions";
    var keep = document.createElement("button");
    keep.className = "fp-pop-btn keep"; keep.type = "button"; keep.textContent = "Keep protected";
    keep.addEventListener("click", function (ev) { ev.stopPropagation(); closeFpPop(); });
    var weakenBtn = document.createElement("button");
    weakenBtn.className = "fp-pop-btn weaken"; weakenBtn.type = "button"; weakenBtn.textContent = "Weaken anyway";
    weakenBtn.addEventListener("click", function (ev) {
      ev.stopPropagation();
      if (step === 1) {
        step = 2;
        warn.innerHTML = "Lower <b>your own</b> protection? You can restore it from this menu at any time.";
        weakenBtn.textContent = "Yes, weaken";
        return;
      }
      closeFpPop();
      onConfirm();
    });
    actions.appendChild(keep); actions.appendChild(weakenBtn);
    fpPop.appendChild(actions);
    fpPop.hidden = false;
  }

  function openFpMenu(e) {
    var d = e.fpData || { fp: 1, inherited: true, reduced: false };
    fpPop.innerHTML = "";
    var title = document.createElement("div");
    title.className = "fp-pop-title";
    title.textContent = "Tracking resistance";
    fpPop.appendChild(title);
    var sub = document.createElement("div");
    sub.className = "fp-pop-sub";
    var now = d.fp === 0 ? "off" : d.reduced ? "reduced" : FP_LEVEL_NAMES[d.fp] || "green";
    sub.textContent = (d.inherited ? "Following the global default" : "Overriding the global") + " · now: " + now;
    fpPop.appendChild(sub);

    // Manual "new identity now" (CD-29 Task D): re-roll THIS window's fingerprint
    // (and its Tor circuit if enabled) and reload it - the "burn it now" control.
    var idBtn = document.createElement("button");
    idBtn.className = "fp-pop-opt fp-newid";
    idBtn.type = "button";
    idBtn.textContent = "↻ New identity now";
    idBtn.title = "Re-roll this window's fingerprint and reload it fresh";
    idBtn.addEventListener("click", function (ev) {
      ev.stopPropagation();
      query({ cmd: "new_identity", slot: e.id }).then(function () {
        idBtn.textContent = "↻ New identity ✓";
        setTimeout(closeFpPop, 700);
      }).catch(function () {});
    });
    fpPop.appendChild(idBtn);

    FP_OPTS.forEach(function (opt) {
      var row = document.createElement("button");
      row.className = "fp-pop-opt"; row.type = "button";
      row.textContent = opt.label;
      var active = opt.level === "inherit" ? d.inherited
        : (!d.inherited && FP_LEVEL_NAMES[d.fp] === opt.level);
      if (active) row.classList.add("active");
      row.addEventListener("click", function (ev) { ev.stopPropagation(); chooseLevel(e, opt.level); });
      fpPop.appendChild(row);
    });
    // Reported screen size for THIS window (CD-29): a compact cycler row. Tapping it
    // advances inherit → 1080p → 900p → 720p → inherit. Every option is a common real
    // resolution (never a decoy), so it is ungated.
    var screenRow = document.createElement("button");
    screenRow.className = "fp-pop-opt fp-screen-row";
    screenRow.type = "button";
    screenRow.textContent = "Screen size: …";
    query({ cmd: "get_slot_screen", slot: e.id }).then(function (r) {
      var d; try { d = JSON.parse(r); } catch (x) { d = null; }
      var label = d ? (d.inherited ? "Global (" + SCREEN_SHORT(d.value) + ")" : SCREEN_SHORT(d.value)) : "…";
      screenRow.textContent = "Screen size: " + label;
    }).catch(function () {});
    screenRow.addEventListener("click", function (ev) {
      ev.stopPropagation();
      cycleSlotScreen(e);
    });
    fpPop.appendChild(screenRow);

    fpPop.hidden = false;
    var r = e.fp.getBoundingClientRect();
    var w = fpPop.offsetWidth || 240;
    fpPop.style.left = Math.min(Math.max(6, r.left), window.innerWidth - w - 6) + "px";
    fpPop.style.top = (r.bottom + 6) + "px";
  }

  // Per-window screen preset (CD-29): the cycle order and short labels.
  var SCREEN_CYCLE = ["inherit", "1920x1080", "1600x900", "1280x720"];
  function SCREEN_SHORT(v) {
    return v === "1920x1080" ? "1080p" : v === "1600x900" ? "900p" : v === "1280x720" ? "720p" : v;
  }
  function cycleSlotScreen(e) {
    query({ cmd: "get_slot_screen", slot: e.id }).then(function (r) {
      var d; try { d = JSON.parse(r); } catch (x) { d = null; }
      var cur = d ? (d.inherited ? "inherit" : d.value) : "inherit";
      var idx = SCREEN_CYCLE.indexOf(cur);
      var next = SCREEN_CYCLE[(idx + 1) % SCREEN_CYCLE.length];
      query({ cmd: "set_slot_screen", slot: e.id, value: next })
        .then(function () { openFpMenu(e); }) // re-open so the row shows the new value
        .catch(function () {});
    }).catch(function () {});
  }

  // A click anywhere else closes the menu (its own clicks stopPropagation).
  document.addEventListener("click", function () { closeFpPop(); });

  // --- Host frame state: position ensembles + reveal the engaged one ------
  // Called by the host on change (execute_java_script): a JSON string of
  //   { slots:[{id,x,w}], engaged:<id|null>, autofocus:<bool> }
  // x/w are DIP in the band's own coordinate space (band origin = window origin).
  window.cdFrame = function (json) {
    var f; try { f = JSON.parse(json); } catch (x) { return; }
    var seen = {};
    var newlyEngaged = f.engaged != null && f.engaged !== engaged;
    var torStatus = f.tor_status | 0; // 0 idle, 1 bootstrapping, 2 ready, 3 failed
    for (var i = 0; i < (f.slots || []).length; i++) {
      var sl = f.slots[i];
      seen[sl.id] = true;
      var e = ensembles[sl.id] || makeEnsemble(sl.id);
      e.el.style.left = sl.x + "px";
      e.el.style.width = sl.w + "px";
      // Tor glyph: lit when this column is on Tor; a pulse while the engine is
      // still bootstrapping (its stream can't route until READY); a distinct lit
      // "ready" state once connected; a warn state if the engine failed to bootstrap
      // (fail-closed - it can't fetch, so a plain lit shield would falsely imply
      // working protection). CD-15 HOTFIX / CD-18.
      e.tor.classList.toggle("on", !!sl.tor);
      e.tor.classList.toggle("connecting", !!sl.tor && torStatus === 1);
      e.tor.classList.toggle("ready", !!sl.tor && torStatus === 2);
      e.tor.classList.toggle("failed", !!sl.tor && torStatus === 3);
      e.tor.setAttribute("aria-pressed", sl.tor ? "true" : "false");
      // Per-window hardening indicator (CD-25): effective level, inherited-vs-override,
      // and a reduced flag (off / below-standard). Driven entirely by the frame push.
      e.fpData = { fp: sl.fp | 0, inherited: sl.fp_inherited !== false, reduced: !!sl.fp_reduced };
      paintFpOrb(e, e.fpData);
      // Close icon: the last window can't be closed (host refuses); dim + disable
      // it there so the UI matches the host behavior (CD-18).
      var lastOne = (f.slots || []).length <= 1;
      e.close.classList.toggle("disabled", lastOne);
      e.close.disabled = lastOne;
    }
    // Drop ensembles for slots that no longer exist.
    for (var id in ensembles) {
      if (!seen[id]) { ensembles[id].el.remove(); delete ensembles[id]; }
    }
    engaged = (f.engaged != null && ensembles[f.engaged]) ? f.engaged : null;
    for (var id2 in ensembles) {
      ensembles[id2].el.classList.toggle("revealed", Number(id2) === engaged);
    }
    launcher.classList.toggle("revealed", engaged != null);
    if (newlyEngaged && ensembles[engaged]) {
      loadState(ensembles[engaged], !!f.autofocus);
      loadLauncher();
    } else if (engaged == null) {
      // Reset per-ensemble typing so a stale value can't wedge the host open.
      for (var id3 in ensembles) { ensembles[id3].lastTyping = false; }
    }
  };

  // Pull the current frame on load (the host also pushes on change).
  query({ cmd: "get_frame" }).then(function (r) { window.cdFrame(r); }).catch(function () {});
})();
