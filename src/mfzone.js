// MF-zone tabbed viewer (CD-18). Talks to the Rust host over the CEF message
// router (window.cefQuery) only — no network, no fetch, no external resources.
// Commands: get_log_lines (incremental via since_seq), tor_status. Wire format in
// docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) { reject("mfzone IPC unavailable"); return; }
      window.cefQuery({
        request: JSON.stringify(req),
        persistent: false,
        onSuccess: function (r) { resolve(r); },
        onFailure: function (code, msg) { reject(msg || ("error " + code)); }
      });
    });
  }

  // Severity ranks (mirror the Rust ring: TRACE=0..ERROR=4). Used for the Log
  // tab's level filter — never trust tracing's inverted Level ordering.
  var SEV = { trace: 0, debug: 1, info: 2, warn: 3, warning: 3, error: 4 };
  function sevOf(level) { return SEV[(level || "").toLowerCase()] != null ? SEV[level.toLowerCase()] : 2; }

  // A line's target belongs to the Tor tab if it is our tor module or one of
  // arti's crates (cyberdesk::tor, tor_dirmgr/guardmgr/chanmgr/proto, arti_client).
  function isTorTarget(t) { return /^(cyberdesk::tor|tor_|arti)/.test(t || ""); }

  var lines = [];            // accumulated ring rows (capped)
  var lastSeq = -1;          // highest seq seen (for incremental get_log_lines)
  var CAP = 2000;

  // --- Tabs ---------------------------------------------------------------
  var tabs = Array.prototype.slice.call(document.querySelectorAll(".tab"));
  var panes = {};
  Array.prototype.slice.call(document.querySelectorAll(".pane")).forEach(function (p) {
    panes[p.getAttribute("data-pane")] = p;
  });
  var active = "tor";
  tabs.forEach(function (t) {
    t.addEventListener("click", function () { setTab(t.getAttribute("data-tab")); });
  });
  function setTab(name) {
    active = name;
    tabs.forEach(function (t) {
      var on = t.getAttribute("data-tab") === name;
      t.classList.toggle("active", on);
      t.setAttribute("aria-selected", on ? "true" : "false");
    });
    Object.keys(panes).forEach(function (k) { panes[k].classList.toggle("active", k === name); });
    // Report the active tab to the host (CD-30 Task A): the Terminal tab renders
    // the MF zone 2× wide and the slot columns reflow; other tabs return it.
    query({ cmd: "mf_tab", tab: name }).catch(function () {});
    render();
  }
  // Sync the host to this page's initial tab (a reloaded view starts back on
  // Tor — the host's wide-terminal state must follow, CD-30).
  query({ cmd: "mf_tab", tab: active }).catch(function () {});

  // --- Streams ------------------------------------------------------------
  var torStream = document.getElementById("tor-stream");
  var logStream = document.getElementById("log-stream");
  var logLevel = document.getElementById("log-level");
  var copyBtn = document.getElementById("log-copy");

  logLevel.addEventListener("change", function () { render(); });
  copyBtn.addEventListener("click", function () {
    var text = shown(logStream === torStream ? isTorTarget : logFilter())
      .map(fmtPlain).join("\n");
    // Clipboard may be unavailable in OSR; fall back silently.
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(flashCopied, function () {});
    }
  });
  function flashCopied() { copyBtn.textContent = "Copied"; setTimeout(function () { copyBtn.textContent = "Copy"; }, 1200); }

  function logFilter() {
    var min = sevOf(logLevel.value);
    return function (l) { return sevOf(l.level) >= min; };
  }
  function shown(pred) { return lines.filter(pred); }

  function two(n) { return (n < 10 ? "0" : "") + n; }
  function fmtTime(ms) {
    var d = new Date(ms);
    return two(d.getHours()) + ":" + two(d.getMinutes()) + ":" + two(d.getSeconds());
  }
  function fmtPlain(l) { return fmtTime(l.ts) + " " + (l.level || "") + " " + (l.target || "") + "  " + (l.msg || ""); }

  // Build one line element with textContent nodes (never innerHTML — a log msg
  // could contain markup; keep it inert).
  function lineEl(l) {
    var lvl = (l.level || "").toLowerCase();
    var row = document.createElement("div");
    row.className = "line lvl-" + lvl;
    var ts = document.createElement("span"); ts.className = "ts"; ts.textContent = fmtTime(l.ts) + " ";
    var badge = document.createElement("span"); badge.className = "badge"; badge.textContent = (l.level || "").padEnd(5) + " ";
    var tgt = document.createElement("span"); tgt.className = "tgt"; tgt.textContent = (l.target || "") + "  ";
    var msg = document.createElement("span"); msg.className = "msg"; msg.textContent = l.msg || "";
    row.appendChild(ts); row.appendChild(badge); row.appendChild(tgt); row.appendChild(msg);
    return row;
  }

  // Auto-scroll: only pin to the bottom if the user was already near the bottom
  // (pause when they scroll up to read; resume when they return to the bottom).
  function nearBottom(el) { return el.scrollHeight - el.scrollTop - el.clientHeight < 24; }
  function fill(el, rows) {
    var pin = nearBottom(el);
    var frag = document.createDocumentFragment();
    if (rows.length === 0) {
      var e = document.createElement("div"); e.className = "empty"; e.textContent = "No lines yet.";
      frag.appendChild(e);
    } else {
      rows.forEach(function (l) { frag.appendChild(lineEl(l)); });
    }
    el.replaceChildren(frag);
    if (pin) el.scrollTop = el.scrollHeight;
  }

  function render() {
    if (active === "tor") fill(torStream, shown(function (l) { return isTorTarget(l.target); }));
    else if (active === "log") fill(logStream, shown(logFilter()));
    // Terminal pane is static.
  }

  // --- Tor status header --------------------------------------------------
  var torHead = document.getElementById("tor-head");
  var torDot = document.getElementById("tor-dot");
  var torState = document.getElementById("tor-state");
  var torReason = document.getElementById("tor-reason");
  var STATE = ["idle", "connecting", "ready", "failed"];
  var LABEL = ["Tor engine idle", "Connecting to the Tor network…", "Connected — Tor ready", "Tor bootstrap failed"];
  function paintTor(status, reason) {
    var s = STATE[status] || "idle";
    torHead.className = "tor-head s-" + s;
    torState.textContent = LABEL[status] || LABEL[0];
    if (status === 3 && reason) { torReason.textContent = reason; torReason.hidden = false; }
    else { torReason.textContent = ""; torReason.hidden = true; }
  }

  // --- Polling ------------------------------------------------------------
  function pollLog() {
    // First poll: OMIT since_seq to get the whole buffer (the ring filters strictly
    // `seq > since_seq`, so sending 0 would drop record seq 0). Later polls are
    // incremental from the highest seq seen.
    var req = lastSeq < 0 ? { cmd: "get_log_lines" } : { cmd: "get_log_lines", since_seq: lastSeq };
    return query(req).then(function (r) {
      var fresh; try { fresh = JSON.parse(r); } catch (x) { return; }
      if (!fresh || !fresh.length) return;
      for (var i = 0; i < fresh.length; i++) {
        if (fresh[i].seq > lastSeq) lastSeq = fresh[i].seq;
        lines.push(fresh[i]);
      }
      if (lines.length > CAP) lines.splice(0, lines.length - CAP);
      render();
    }).catch(function () {});
  }
  function pollTor() {
    return query({ cmd: "tor_status" }).then(function (r) {
      var st = 0, reason = "";
      try { var j = JSON.parse(r); st = j.status | 0; reason = j.reason || ""; } catch (x) {}
      paintTor(st, reason);
    }).catch(function () {});
  }

  function tick() { pollLog(); pollTor(); }
  tick();
  setInterval(tick, 1000);

  // --- Application quit (CD-21) -------------------------------------------
  // Two host commands: `quit` (no save) and `quit_save` (persist the session, then
  // quit). The host queues the request and the main thread exits the loop. Distinct
  // from the per-slot `close_slot` (CD-18).
  var quitBtn = document.getElementById("quit");
  var quitSaveBtn = document.getElementById("quit-save");
  if (quitBtn) {
    quitBtn.addEventListener("click", function () { query({ cmd: "quit" }).catch(function () {}); });
  }
  if (quitSaveBtn) {
    quitSaveBtn.addEventListener("click", function () { query({ cmd: "quit_save" }).catch(function () {}); });
  }
})();
