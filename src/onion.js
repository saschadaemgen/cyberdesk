// Onion refusal page (CD-35 Task B). Talks to the Rust host over the CEF
// message router (window.cefQuery) only - no network, no fetch, no external
// resources. The refused target and the slot id arrive in the query string
// (?s=<slot>&u=<encoded url>), stamped by the host when it built this URL, so
// the buttons act on the window they live in. The host re-validates everything
// (the URL must really be an http(s) .onion, the Tor master switch must be on).

(function () {
  "use strict";

  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) { reject("IPC unavailable"); return; }
      window.cefQuery({
        request: JSON.stringify(req),
        persistent: false,
        onSuccess: function (r) { resolve(r); },
        onFailure: function (code, msg) { reject(msg || ("error " + code)); }
      });
    });
  }

  // URLSearchParams.get already percent-decodes the strictly-encoded value
  // (the host encodes every reserved byte, so no literal '+' ever reaches the
  // plus-to-space rule) - decoding again here would corrupt a target that
  // legitimately contains %XX sequences.
  var params = new URLSearchParams(location.search);
  var target = params.get("u") || "";
  var slot = parseInt(params.get("s") || "", 10);

  var addrEl = document.getElementById("addr");
  var errEl = document.getElementById("err");
  // textContent only - the target is untrusted input and must never be markup.
  addrEl.textContent = target || "(no address)";
  addrEl.title = target;

  function fail(msg) {
    errEl.textContent = msg;
    errEl.hidden = false;
  }

  function act(cmd) {
    errEl.hidden = true;
    if (!target) { fail("No address to open."); return; }
    var req = { cmd: cmd, url: target };
    if (!isNaN(slot)) { req.slot = slot; }
    query(req).catch(function (msg) { fail(String(msg)); });
  }

  document.getElementById("open-tor").addEventListener("click", function () {
    act("onion_open_tor");
  });
  document.getElementById("switch-tor").addEventListener("click", function () {
    act("onion_switch_tor");
  });
})();
