// Vault lock page logic (CD-40, D-0058; unlock model per CD-42, D-0062).
// Renders the vault state the host pushes (window.cdVault) / serves on load
// (get_vault_state). Two modes from the same push: unlock (a vault exists)
// and mandatory first-launch setup (none does). No secret ever reaches this
// page: the host consumes the keyboard while capturing and this page draws
// dots from a character COUNT. Wire format: docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  var titleEl = document.getElementById("title");
  var subtitleEl = document.getElementById("subtitle");
  var hintEl = document.getElementById("hint");
  var entryEl = document.getElementById("entry");
  var dotsEl = document.getElementById("dots");
  var statusEl = document.getElementById("status");
  var consequenceEl = document.getElementById("consequence");
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

  function renderDots(n) {
    // Cap the DOM at a sane count; past 64 the count alone is shown.
    var cap = Math.min(n, 64);
    while (dotsEl.children.length > cap) dotsEl.removeChild(dotsEl.lastChild);
    while (dotsEl.children.length < cap) {
      var d = document.createElement("span");
      d.className = "dot";
      dotsEl.appendChild(d);
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
    renderDots(s.chars || 0);
    entryEl.classList.toggle("busy", !!s.busy);

    if (s.broken) {
      titleEl.textContent = "Vault unavailable";
      subtitleEl.textContent = "Start authorization";
      hintEl.textContent =
        "The vault file failed to validate. CyberDesk stays locked rather than " +
        "guessing — the details below say what to do.";
      setStatus(s.broken, false);
      consequenceEl.hidden = true;
      return;
    }

    var setup = s.capture === "setup_pass" || s.capture === "setup_confirm";
    if (setup) {
      titleEl.textContent = "Set your master password";
      subtitleEl.textContent = "First launch";
      hintEl.textContent = s.capture === "setup_confirm"
        ? "Re-type the master password to confirm — Enter creates the vault."
        : "CyberDesk requires a master password before anything else starts. " +
          "It is typed into the CyberDesk core, not this page — Enter to continue.";
      consequenceEl.hidden = false;
    } else {
      titleEl.textContent = "Vault locked";
      subtitleEl.textContent = "Start authorization";
      hintEl.textContent =
        "Enter your master password. Keystrokes go to the CyberDesk core only — " +
        "no page ever sees them.";
      consequenceEl.hidden = true;
    }

    if (s.busy) {
      setStatus(setup ? "Creating the vault…" : "Checking…", true);
    } else if (s.error) {
      setStatus(s.error, false);
    } else {
      setStatus(null);
    }
  }

  window.cdVault = function (json) {
    try { render(JSON.parse(json)); } catch (e) { /* keep last good state */ }
  };

  quitEl.addEventListener("click", function () {
    query({ cmd: "quit" }).catch(function () {});
  });

  query({ cmd: "get_vault_state" }).then(function (r) {
    try { render(JSON.parse(r)); } catch (e) {}
  }).catch(function (e) { setStatus(String(e), false); });
})();
