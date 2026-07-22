// Vault lock page logic (CD-40/CD-42/CD-43; flow + ergonomics CD-44 A1/A2).
// Renders the vault state the host pushes (window.cdVault) / serves on load
// (get_vault_state). Two modes from the same push: unlock (a vault exists)
// and mandatory first-launch setup (none does). No secret ever reaches this
// page: the host consumes the keyboard while capturing and this page draws
// dots from a character COUNT. The entry is ALWAYS focused while this page
// is up (the host owns the keyboard); the UI shows that instead of
// pretending to be an input. Wire format: docs/cyberdesk-wire-format.md.

(function () {
  "use strict";

  var titleEl = document.getElementById("title");
  var subtitleEl = document.getElementById("subtitle");
  var hintEl = document.getElementById("hint");
  var entryEl = document.getElementById("entry");
  var dotsEl = document.getElementById("dots");
  var statusEl = document.getElementById("status");
  var consequenceEl = document.getElementById("consequence");
  var placeholderEl = document.getElementById("placeholder");
  var footEl = document.getElementById("foot");
  var quitEl = document.getElementById("quit");
  var meterEl = document.getElementById("meter");
  var meterFill = document.getElementById("meter-fill");
  var meterLabel = document.getElementById("meter-label");
  var critLen = document.getElementById("crit-len");
  var meterFb = document.getElementById("meter-fb");
  var weakEl = document.getElementById("weak");
  var weakUse = document.getElementById("weak-use");
  var offerEl = document.getElementById("offer");
  var offerAdd = document.getElementById("offer-add");
  var offerSkip = document.getElementById("offer-skip");

  // The host's zxcvbn score 0..4, verbalized (D-0044: confident, accurate).
  var SCORE_LABELS = ["Very weak", "Weak", "Fair", "Strong", "Very strong"];

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

  // Render the live meter from the HOST-computed snapshot (score, criteria,
  // canned feedback). The password itself never reaches this page. The weak
  // block follows ONLY the host's staged weak_pending, so the meter, the
  // criteria and the warning can never disagree (CD-44 A2).
  function renderMeter(s) {
    var st = s.strength;
    var show = !!st && (s.capture === "setup_pass" || s.capture === "change_pass");
    meterEl.hidden = !show;
    weakEl.hidden = !(show && s.weak_pending);
    if (!show) return;
    meterEl.className = "meter s" + st.score;
    meterFill.style.width = s.chars ? (((st.score + 1) * 20) + "%") : "0";
    meterLabel.textContent = s.chars ? SCORE_LABELS[st.score] : " ";
    critLen.textContent = (st.len_ok ? "✓ " : "") + st.target_len + "+ characters";
    critLen.className = st.len_ok ? "crit met" : "crit";
    var fb = [];
    if (st.warning) fb.push(st.warning);
    if (st.suggestions && st.suggestions.length) fb = fb.concat(st.suggestions);
    meterFb.hidden = !fb.length;
    meterFb.textContent = fb.join(" ");
  }

  function render(s) {
    renderDots(s.chars || 0);
    entryEl.classList.toggle("busy", !!s.busy);
    // The entry is live (host-captured, focused) whenever a capture is open
    // and no worker is running - the visual focus never lies (CD-44 A1).
    entryEl.classList.toggle("live", !!s.capture && !s.busy);
    renderMeter(s);

    // The first-run passkey offer (CD-44 D1): the vault is already set up,
    // so the entry and the meter step aside for one optional question.
    if (s.offer_passkey) {
      offerEl.hidden = false;
      entryEl.hidden = true;
      meterEl.hidden = true;
      weakEl.hidden = true;
      consequenceEl.hidden = true;
      placeholderEl.hidden = true;
      titleEl.textContent = "Master password set";
      subtitleEl.textContent = "First launch - optional step";
      hintEl.textContent =
        "Your vault is ready. One optional extra: a passkey as the second factor.";
      footEl.textContent = "";
      var busyHello = s.hello === "enroll";
      offerAdd.disabled = busyHello;
      offerAdd.textContent = busyHello ? "Follow Windows Hello…" : "Set up passkey";
      offerSkip.disabled = busyHello;
      setStatus(s.error || (busyHello
        ? "Confirm twice with Windows Hello: once to create the passkey, once to derive its vault secret."
        : null), !s.error);
      return;
    }
    offerEl.hidden = true;
    entryEl.hidden = false;

    if (s.broken) {
      titleEl.textContent = "Vault unavailable";
      subtitleEl.textContent = "Start authorization";
      hintEl.textContent =
        "The vault file failed to validate. CyberDesk stays locked rather than " +
        "guessing; the details below say what to do.";
      setStatus(s.broken, false);
      consequenceEl.hidden = true;
      placeholderEl.hidden = true;
      meterEl.hidden = true;
      weakEl.hidden = true;
      footEl.textContent = "";
      return;
    }

    var twofa = s.required === 2;
    var placeholder = "";
    var foot = "Enter continues · Backspace edits · Esc clears the entry · Ctrl+V pastes";

    switch (s.capture) {
      case "setup_pass":
        titleEl.textContent = "Set your master password";
        subtitleEl.textContent = "First launch · step 1 of 2";
        hintEl.textContent =
          "Choose the password that protects CyberDesk. The field below is " +
          "already focused: what you type goes straight into the CyberDesk " +
          "core, never to any page.";
        consequenceEl.hidden = false;
        placeholder = "Type your master password";
        break;
      case "setup_confirm":
        titleEl.textContent = "Set your master password";
        subtitleEl.textContent = "First launch · step 2 of 2";
        hintEl.textContent =
          "Re-type the same password to confirm it. Enter creates the vault.";
        consequenceEl.hidden = false;
        placeholder = "Re-type your master password";
        foot = "Enter creates the vault · Esc on an empty field goes back one step";
        break;
      case "change_pass":
        titleEl.textContent = "Change your master password";
        subtitleEl.textContent = "Step 1 of 2";
        hintEl.textContent = "Type the new master password.";
        consequenceEl.hidden = true;
        placeholder = "Type the new master password";
        break;
      case "change_confirm":
        titleEl.textContent = "Change your master password";
        subtitleEl.textContent = "Step 2 of 2";
        hintEl.textContent = "Re-type the new password to confirm it.";
        consequenceEl.hidden = true;
        placeholder = "Re-type the new password";
        foot = "Enter applies · Esc on an empty field goes back one step";
        break;
      default:
        titleEl.textContent = "Vault locked";
        subtitleEl.textContent = twofa
          ? "Start authorization · two-factor"
          : "Start authorization";
        hintEl.textContent = twofa
          ? "Two-factor unlock: enter your master password, then confirm with " +
            "Windows Hello. The field is already focused; keystrokes go to " +
            "the CyberDesk core only."
          : "Enter your master password. The field below is already focused: " +
            "what you type goes straight into the CyberDesk core, never to " +
            "any page.";
        consequenceEl.hidden = true;
        placeholder = "Enter your master password";
        foot = "Enter unlocks · Backspace edits · Esc clears the entry · Ctrl+V pastes";
        break;
    }

    // The placeholder is the neutral empty state (never a verdict).
    placeholderEl.textContent = placeholder;
    placeholderEl.hidden = (s.chars || 0) > 0 || !s.capture || !!s.busy;
    footEl.textContent = foot;

    if (s.hello === "assert") {
      // The host holds the Hello modal open - the second factor (CD-43).
      setStatus("Second factor: confirm with Windows Hello…", true);
    } else if (s.busy) {
      var setupish = s.capture === "setup_confirm" || s.capture === "setup_pass";
      setStatus(setupish ? "Creating the vault…" : "Checking…", true);
    } else if (s.error) {
      setStatus(s.error, false);
    } else {
      setStatus(null);
    }
  }

  window.cdVault = function (json) {
    try { render(JSON.parse(json)); } catch (e) { /* keep last good state */ }
  };

  // Click-to-focus, honestly: the entry is captured by the host the whole
  // time, so a click acknowledges the focus visually instead of moving it.
  entryEl.addEventListener("mousedown", function () {
    entryEl.classList.remove("pulse");
    // Force a reflow so the animation restarts on every click.
    void entryEl.offsetWidth;
    entryEl.classList.add("pulse");
  });

  weakUse.addEventListener("click", function () {
    query({ cmd: "vault_accept_weak" }).then(function (r) {
      try { render(JSON.parse(r)); } catch (e) {}
    }).catch(function (e) { setStatus(String(e), false); });
  });

  offerAdd.addEventListener("click", function () {
    query({ cmd: "vault_enroll_passkey" }).then(function (r) {
      try { render(JSON.parse(r)); } catch (e) {}
    }).catch(function (e) { setStatus(String(e), false); });
  });

  offerSkip.addEventListener("click", function () {
    query({ cmd: "vault_skip_passkey_offer" }).then(function (r) {
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
