// Command bar logic (CD-08 hover-reveal top bar). Talks to the Rust host over
// the CEF message router (window.cefQuery) only — no network, no fetch. The host
// classifies input (URL vs search), ranks suggestions from favorites + history,
// drives the surf view, and owns the reveal/hide animation. This page renders
// what it is given and reports two things the host needs: whether the user is
// actively typing (so a mouse-out does not interrupt them) and — implicitly, via
// the query text — whether to show favorite chips or the live suggestion list.
// Wire format: see docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) {
        reject("command IPC unavailable");
        return;
      }
      window.cefQuery({
        request: JSON.stringify(req),
        persistent: false,
        onSuccess: function (r) { resolve(r); },
        onFailure: function (c, m) { reject(m || ("error " + c)); }
      });
    });
  }

  var input = document.getElementById("url");
  var scheme = document.getElementById("scheme");
  var star = document.getElementById("star");
  var chips = document.getElementById("chips");
  var list = document.getElementById("suggestions");

  // The current surf page (from nav state) — the star and Ctrl+D act on this,
  // not on the typed input.
  var currentUrl = "";
  var currentTitle = "";

  // The URL the input is prefilled with on open. While the input still holds it
  // untouched (or is empty), the bar shows the favorites chips; the first
  // keystroke replaces the selected text and switches to the live suggestion
  // list. (This also fixes the CD-07 "only one favorite" bug — the favorites
  // surface is no longer filtered by the current address.)
  var pristineUrl = "";

  // Live suggestions and the keyboard selection (-1 = the raw input, no row).
  var suggestions = [];
  var selIndex = -1;
  var debounceTimer = null;

  // Last "actively typing" state reported to the host (focus + a real typed
  // query), so the host's mouse-out hide skips a user mid-search.
  var lastTyping = false;

  var STAR_PATH =
    "M12 3.6l2.6 5.27 5.82.85-4.21 4.1.99 5.79L12 16.87l-5.2 2.74.99-5.79-4.21-4.1 5.82-.85z";

  function applyScheme(s) {
    var cls = s === "https" ? "secure" : (s === "http" ? "insecure" : "neutral");
    scheme.className = "scheme " + cls;
  }

  function paintStar(fav) {
    star.classList.toggle("on", !!fav);
    star.setAttribute("aria-pressed", fav ? "true" : "false");
  }

  // Apply a nav-state snapshot: scheme hint, current page, star state.
  function applyNavState(s) {
    currentUrl = s.url || "";
    currentTitle = s.title || "";
    applyScheme(s.scheme);
    paintStar(s.favorite);
  }

  function refreshState() {
    return query({ cmd: "get_nav_state" }).then(function (r) {
      return JSON.parse(r);
    });
  }

  function navigateTo(value) {
    query({ cmd: "navigate", input: value });
    // The host closes the bar and navigates the surf view.
  }

  // Tell the host whether the input is engaged (focused and holding text — the
  // prefilled URL counts, so a keyboard user is never cut off mid-edit). While
  // this holds, a mouse-out does not hide the bar. Only reported on change.
  function reportTyping() {
    var active = document.activeElement === input && input.value.length > 0;
    if (active !== lastTyping) {
      lastTyping = active;
      query({ cmd: "bar_typing", active: active }).catch(function () { /* ignore */ });
    }
  }

  // --- Favorites chips ----------------------------------------------------

  function renderChips(items) {
    chips.textContent = "";
    for (var i = 0; i < items.length; i++) {
      var f = items[i];
      var chip = document.createElement("button");
      chip.className = "chip";
      chip.title = f.url;
      // Static markup (no user data) — safe to insert as HTML.
      chip.insertAdjacentHTML(
        "beforeend",
        '<svg class="chip-star" viewBox="0 0 24 24"><path d="' +
          STAR_PATH + '" fill="currentColor"/></svg>'
      );
      var t = document.createElement("span");
      t.className = "chip-title";
      t.textContent = (f.title && f.title.length) ? f.title : f.url;
      chip.appendChild(t);
      (function (url) {
        chip.addEventListener("mousedown", function (e) { e.preventDefault(); });
        chip.addEventListener("click", function () { navigateTo(url); });
      })(f.url);
      chips.appendChild(chip);
    }
  }

  // --- Suggestions --------------------------------------------------------

  function updateSelection() {
    var rows = list.children;
    for (var i = 0; i < rows.length; i++) {
      rows[i].classList.toggle("sel", i === selIndex);
    }
    if (selIndex >= 0 && rows[selIndex]) {
      rows[selIndex].scrollIntoView({ block: "nearest" });
    }
  }

  function renderSuggestions(items) {
    suggestions = items || [];
    selIndex = -1;
    list.textContent = "";
    for (var i = 0; i < suggestions.length; i++) {
      var s = suggestions[i];
      var row = document.createElement("div");
      row.className = "suggest";
      row.setAttribute("role", "option");
      if (s.favorite) {
        row.insertAdjacentHTML(
          "beforeend",
          '<svg class="row-star" viewBox="0 0 24 24"><path d="' +
            STAR_PATH + '" fill="currentColor"/></svg>'
        );
      }
      var hasTitle = !!(s.title && s.title.length);
      var tEl = document.createElement("span");
      tEl.className = "row-title";
      tEl.textContent = hasTitle ? s.title : s.url;
      row.appendChild(tEl);
      if (hasTitle) {
        var uEl = document.createElement("span");
        uEl.className = "row-url";
        uEl.textContent = s.url;
        row.appendChild(uEl);
      }
      (function (url) {
        row.addEventListener("mousedown", function (e) { e.preventDefault(); });
        row.addEventListener("click", function () { navigateTo(url); });
      })(s.url);
      list.appendChild(row);
    }
  }

  // Text to query for: empty (favorites) while the input still holds the
  // untouched prefilled URL, otherwise the live typed value.
  function suggestQueryText() {
    return input.value === pristineUrl ? "" : input.value;
  }

  // Fetch the body for the current input: empty -> favorites shown as chips (the
  // suggestion list cleared); typed -> live suggestions (chips cleared). Only one
  // is ever non-empty, which is exactly what the host sizes the bar for.
  function runSuggest() {
    var q = suggestQueryText();
    query({ cmd: "query_suggestions", input: q })
      .then(function (r) {
        var items;
        try { items = JSON.parse(r); } catch (e) { items = []; }
        if (q === "") {
          renderChips(items);
          renderSuggestions([]);
        } else {
          renderChips([]);
          renderSuggestions(items);
        }
      })
      .catch(function () { renderChips([]); renderSuggestions([]); });
  }

  // Debounced per-keystroke query (~90 ms) — trivial indexed lookups, but do
  // not spam them (D-0014 guidance).
  function scheduleSuggest() {
    if (debounceTimer) clearTimeout(debounceTimer);
    debounceTimer = setTimeout(runSuggest, 90);
  }

  // --- Favorites toggle ---------------------------------------------------

  function toggleFavorite() {
    if (!currentUrl) return;
    query({ cmd: "toggle_favorite", url: currentUrl, title: currentTitle })
      .then(function (r) {
        try { paintStar(JSON.parse(r).favorite); } catch (e) { /* ignore */ }
        runSuggest(); // the chips / favorite-history split may have changed
      })
      .catch(function () { /* ignore */ });
  }

  // --- Wiring -------------------------------------------------------------

  // Load the current nav state: prefill the URL, set scheme + star, focus only
  // when the host asks (Ctrl+L), and show favorites for the untouched input.
  refreshState()
    .then(function (s) {
      applyNavState(s);
      pristineUrl = s.url || "";
      input.value = pristineUrl;
      if (s.autofocus) { input.focus(); input.select(); }
      runSuggest(); // input untouched -> shows the favorites chips
    })
    .catch(function () { runSuggest(); });

  input.addEventListener("input", function () { scheduleSuggest(); reportTyping(); });
  input.addEventListener("focus", reportTyping);
  input.addEventListener("blur", reportTyping);

  input.addEventListener("keydown", function (e) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (suggestions.length) {
        selIndex = selIndex + 1;
        if (selIndex >= suggestions.length) selIndex = -1;
        updateSelection();
      }
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      if (suggestions.length) {
        selIndex = selIndex - 1;
        if (selIndex < -1) selIndex = suggestions.length - 1;
        updateSelection();
      }
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (selIndex >= 0 && suggestions[selIndex]) {
        navigateTo(suggestions[selIndex].url);
      } else {
        navigateTo(input.value);
      }
    }
  });

  star.addEventListener("click", toggleFavorite);

  // Ctrl+D toggles the current page's favorite while the bar is open (the
  // surf-view Ctrl+D is handled host-side). The star updates live.
  document.addEventListener("keydown", function (e) {
    if ((e.ctrlKey || e.metaKey) && (e.key === "d" || e.key === "D")) {
      e.preventDefault();
      toggleFavorite();
    }
  });

  // Back / Forward / Reload glyphs (surf-view navigation), then refresh state.
  var buttons = document.querySelectorAll(".nav .glyph");
  for (var i = 0; i < buttons.length; i++) {
    (function (b) {
      b.addEventListener("click", function () {
        query({ cmd: b.dataset.act }).then(function () {
          refreshState().then(applyNavState).catch(function () {});
        }).catch(function () {});
      });
    })(buttons[i]);
  }
})();
