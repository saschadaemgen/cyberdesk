// CyberDesk start page logic (CD-14). Talks to the Rust host ONLY over the CEF
// message router (window.cefQuery) - no network, no fetch, no external resources.
// The search box reuses `navigate` (host-side URL-vs-search + search_engine); the
// favorite tiles reuse `query_suggestions ""` and `navigate`. Both act on this
// slot (interacting with it makes it the active slot host-side). Wire format:
// docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  var input = document.getElementById("q");
  var form = document.getElementById("search");
  var tilesEl = document.getElementById("tiles");

  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) {
        reject("start IPC unavailable");
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

  // Navigate this slot (the host classifies URL vs search and loads the active
  // slot; interacting with the start page has already made it active).
  function navigate(text) {
    var t = (text || "").trim();
    if (!t) return;
    query({ cmd: "navigate", input: t }).catch(function () {});
  }

  form.addEventListener("submit", function (ev) {
    ev.preventDefault();
    navigate(input.value);
  });

  // Favorite tiles - the top favorites, click to open in this slot.
  function initial(title, url) {
    var s = (title && title.trim()) || url.replace(/^https?:\/\/(www\.)?/, "");
    var c = (s || "?").trim().charAt(0);
    return c ? c.toUpperCase() : "?";
  }

  function loadTiles() {
    query({ cmd: "query_suggestions", input: "" }).then(function (r) {
      var items; try { items = JSON.parse(r); } catch (x) { items = []; }
      tilesEl.textContent = "";
      items.forEach(function (f) {
        var tile = document.createElement("button");
        tile.className = "tile";
        tile.type = "button";
        tile.title = (f.title && f.title.length) ? f.title : f.url;
        tile.textContent = initial(f.title, f.url);
        tile.addEventListener("click", function () { navigate(f.url); });
        tilesEl.appendChild(tile);
      });
    }).catch(function () {});
  }

  loadTiles();
})();
