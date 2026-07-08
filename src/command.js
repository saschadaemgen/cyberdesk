// Command bar logic. Talks to the Rust host over the CEF message router
// (window.cefQuery) only — no network, no fetch. The host decides URL vs search
// and performs the navigation on the surf view. Wire format: see
// docs/cyberdesk-wire-format.md.

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

  function applyScheme(s) {
    var cls = s === "https" ? "secure" : (s === "http" ? "insecure" : "neutral");
    scheme.className = "scheme " + cls;
  }

  function refreshState() {
    return query({ cmd: "get_nav_state" }).then(function (r) {
      return JSON.parse(r);
    });
  }

  // Load the current nav state: prefill + select the URL, set the scheme hint.
  refreshState()
    .then(function (s) {
      input.value = s.url || "";
      applyScheme(s.scheme);
      input.focus();
      input.select();
    })
    .catch(function () { input.focus(); });

  input.addEventListener("keydown", function (e) {
    if (e.key === "Enter") {
      e.preventDefault();
      query({ cmd: "navigate", input: input.value });
      // The host closes the bar and navigates the surf view.
    }
  });

  var buttons = document.querySelectorAll(".glyph");
  for (var i = 0; i < buttons.length; i++) {
    (function (b) {
      b.addEventListener("click", function () {
        query({ cmd: b.dataset.act }).then(function () {
          refreshState().then(function (s) { applyScheme(s.scheme); }).catch(function () {});
        }).catch(function () {});
      });
    })(buttons[i]);
  }
})();
