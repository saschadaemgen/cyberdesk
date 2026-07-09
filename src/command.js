// Floating command sets (CD-12, D-0021). The CD-08 single bar is gone: this page
// draws N per-window ensembles on a TRANSPARENT body plus one shared favorites
// launcher. The HOST positions each ensemble above its column and tells the page
// which slot is engaged (hovered) via window.cdFrame(json); the page reveals that
// ensemble (CSS fade+drop, ~220 ms) and binds its capsule / orbs / star / star to
// THAT column. All logic (suggestions, star, scheme, nav) is reused from CD-07/08
// per capsule; only the presentation and per-slot binding changed. Talks to the
// host over the CEF message router only (window.cefQuery) — no network.
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
    star: '<svg viewBox="0 0 24 24" width="17" height="17"><path class="star-path" d="' + STAR_PATH + '" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round"/></svg>'
  };

  var band = document.getElementById("band");
  var launcher = document.getElementById("launcher");
  var ensembles = {};   // slot id -> ensemble state
  var engaged = null;   // engaged (hovered) slot id, or null

  // --- One per-window ensemble --------------------------------------------

  function makeEnsemble(id) {
    var el = document.createElement("div");
    el.className = "ensemble";
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
      '</div>' +
      '<div class="suggestions" role="listbox" aria-label="Suggestions"></div>';
    band.appendChild(el);

    var e = {
      id: id, el: el,
      input: el.querySelector(".url"),
      scheme: el.querySelector(".scheme"),
      star: el.querySelector(".star"),
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

  // --- Host frame state: position ensembles + reveal the engaged one ------
  // Called by the host on change (execute_java_script): a JSON string of
  //   { slots:[{id,x,w}], engaged:<id|null>, autofocus:<bool> }
  // x/w are DIP in the band's own coordinate space (band origin = window origin).
  window.cdFrame = function (json) {
    var f; try { f = JSON.parse(json); } catch (x) { return; }
    var seen = {};
    var newlyEngaged = f.engaged != null && f.engaged !== engaged;
    for (var i = 0; i < (f.slots || []).length; i++) {
      var sl = f.slots[i];
      seen[sl.id] = true;
      var e = ensembles[sl.id] || makeEnsemble(sl.id);
      e.el.style.left = sl.x + "px";
      e.el.style.width = sl.w + "px";
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
