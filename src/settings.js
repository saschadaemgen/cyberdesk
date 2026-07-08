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

  // Load current values on startup.
  query({ cmd: "get_settings" })
    .then(function (response) {
      var s = JSON.parse(response);
      paint("feather_edges", s.feather_edges);
      paint("deep_field", s.deep_field);
      paint("stay_foreground", s.stay_foreground);
    })
    .catch(function (err) { setStatus(String(err), true); });
})();
