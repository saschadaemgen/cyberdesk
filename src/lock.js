// Vault lock page logic (CD-40, D-0058). Renders the vault state the host
// pushes (window.cdVault) / serves on load (get_vault_state). No secret ever
// reaches this page: the host consumes the keyboard while capturing and this
// page draws dots from a character COUNT. Wire format:
// docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  var titleEl = document.getElementById("title");
  var hintEl = document.getElementById("hint");
  var entryEl = document.getElementById("entry");
  var dotsEl = document.getElementById("dots");
  var statusEl = document.getElementById("status");
  var altEl = document.getElementById("alt");
  var quitEl = document.getElementById("quit");

  function query(req) {
    return new Promise(function (resolve, reject) {
      if (!window.cefQuery) {
        reject("vault IPC unavailable");
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

  // The current capture purpose, mirrored from the last push (drives the
  // alternate-method button label and the dot grouping).
  var current = { capture: null, chars: 0 };

  function renderDots(n, grouped) {
    // Cap the DOM at a sane count; past 64 the count alone is shown.
    var cap = Math.min(n, 64);
    while (dotsEl.children.length > cap) dotsEl.removeChild(dotsEl.lastChild);
    while (dotsEl.children.length < cap) {
      var d = document.createElement("span");
      d.className = "dot";
      dotsEl.appendChild(d);
    }
    for (var i = 0; i < dotsEl.children.length; i++) {
      // Recovery keys read as 14 groups of 4 — group the dots the same way.
      dotsEl.children[i].className = grouped && (i % 4 === 3) ? "dot g" : "dot";
    }
  }

  function setStatus(text, isInfo) {
    if (!text) {
      statusEl.hidden = true;
      return;
    }
    statusEl.hidden = false;
    statusEl.textContent = text;
    statusEl.className = isInfo ? "status info" : "status";
  }

  function render(s) {
    current = s;
    var grouped = s.capture === "unlock_recovery";
    renderDots(s.chars || 0, grouped);
    entryEl.classList.toggle("busy", !!s.busy);

    if (s.broken) {
      titleEl.textContent = "Vault unavailable";
      hintEl.textContent =
        "The vault file failed to validate. CyberDesk stays locked rather than " +
        "guessing — restore vault.json from your own backup, then relaunch.";
      setStatus(s.broken, false);
      altEl.hidden = true;
      return;
    }

    if (s.busy) {
      setStatus("Checking…", true);
    } else if (s.error) {
      setStatus(s.error, false);
    } else if (s.step2) {
      setStatus("Passphrase accepted for this attempt — now the recovery key (2-required policy).", true);
    } else {
      setStatus(null);
    }

    if (s.capture === "unlock_recovery") {
      titleEl.textContent = "Vault locked";
      hintEl.textContent = s.step2
        ? "Second factor: enter the recovery key (14 groups, dashes optional). Ctrl+V pastes via the core."
        : "Enter the recovery key (14 groups, dashes optional). Ctrl+V pastes via the core.";
      altEl.textContent = "Use the passphrase";
      altEl.hidden = !!s.step2;
    } else {
      titleEl.textContent = "Vault locked";
      hintEl.textContent =
        "Enter your passphrase. Keystrokes go to the CyberDesk core only — no page ever sees them.";
      altEl.textContent = "Use the recovery key";
      altEl.hidden = false;
    }
  }

  window.cdVault = function (json) {
    try { render(JSON.parse(json)); } catch (e) { /* keep last good state */ }
  };

  altEl.addEventListener("click", function () {
    var next = current.capture === "unlock_recovery" ? "unlock_pass" : "unlock_recovery";
    query({ cmd: "vault_begin_capture", purpose: next }).then(function (r) {
      try { render(JSON.parse(r)); } catch (e) {}
    }).catch(function (e) { setStatus(String(e), false); });
  });

  quitEl.addEventListener("click", function () {
    query({ cmd: "quit" }).catch(function () {});
  });

  query({ cmd: "get_vault_state" }).then(function (r) {
    try { render(JSON.parse(r)); } catch (e) {}
  }).catch(function (e) { setStatus(String(e), false); });
})();
