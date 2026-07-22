// Info panel logic (CD-13 → CD-22). Talks to the Rust host exclusively over the CEF
// message router (window.cefQuery) - no network, no fetch, no external resources.
// Read-only: it asks the host for the component statuses and renders them. The status
// for every external dependency is derived CLIENT-SIDE (installed vs a build-declared
// latest-known version); there is no live manifest fetch and no "Last check failed"
// footer (retired in CD-22 - the app self-update feed returns in its own later ticket).
// Only command: get_info_items. Wire format in docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  var compsEl = document.getElementById("components");

  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) {
        reject("info IPC unavailable");
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

  function el(tag, cls, text) {
    var e = document.createElement(tag);
    if (cls) e.className = cls;
    if (text != null) e.textContent = text;
    return e;
  }

  // --- Component list: real per-component status (CD-22) ---------------------
  // The state map is the single place the vocabulary lives, so the wording stays
  // consistent and never claims more than the host reported. "informational" is only
  // a defensive fallback for an undeclared component - every tracked one has a real
  // comparison result (up to date / update available / held back).
  var STATE = {
    current:       { cls: "ok",     label: "Up to date" },
    update:        { cls: "update", label: "Update available" },
    held_back:     { cls: "held",   label: "Held back" },
    informational: { cls: "info",   label: "Installed" }
  };

  function renderComponent(c) {
    var status = c.status || "informational";
    var meta = STATE[status] || STATE.informational;

    var card = el("div", "comp comp-" + status);

    var head = el("div", "comp-head");
    head.appendChild(el("span", "comp-name", c.name || c.id || "?"));
    head.appendChild(el("span", "comp-state " + meta.cls, meta.label));
    card.appendChild(head);

    // Installed version (+ optional secondary detail, e.g. "Chromium 149.x").
    var ver = "Installed " + (c.version || "?");
    if (c.detail) ver += " · " + c.detail;
    card.appendChild(el("div", "comp-ver", ver));

    // Upstream line - only where there is something honest to say.
    if (status === "update" && c.latest) {
      card.appendChild(el("div", "comp-upstream update", "Version " + c.latest + " available"));
    } else if (status === "held_back" && c.latest) {
      card.appendChild(el("div", "comp-upstream held", "Newest release " + c.latest + " - deliberately not installed"));
    }

    // Held-back explanation: why we hold it, and what unpins it. Reads as an
    // intentional decision, never as an error or a pending user action.
    if (status === "held_back") {
      if (c.reason) card.appendChild(el("div", "comp-reason", c.reason));
      if (c.note) card.appendChild(el("div", "comp-note", c.note));
    }

    return card;
  }

  function render(snap) {
    compsEl.replaceChildren();
    var comps = (snap && snap.components) || [];
    if (comps.length === 0) {
      compsEl.appendChild(el("p", "empty", "No component information available."));
      return;
    }
    comps.forEach(function (c) { compsEl.appendChild(renderComponent(c)); });
  }

  function load() {
    return query({ cmd: "get_info_items" })
      .then(function (resp) { render(JSON.parse(resp)); })
      .catch(function () {
        compsEl.replaceChildren(el("p", "empty", "Component information unavailable."));
      });
  }

  load();
})();
