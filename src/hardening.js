// CyberDesk — fingerprinting hardening (CD-16, D-0039).
//
// COHERENT, PER-SESSION TRACKING-RESISTANCE — NOT anonymity, NOT OS/UA/platform
// spoofing (binding constraint EC-01). The goal is to break a site's ability to
// LINK this browser across sites and across sessions, without introducing a single
// cross-surface contradiction. We deliberately do NOT touch the User-Agent,
// navigator.platform / oscpu, CPU/OS strings, or language — leaving them real and
// mutually consistent. Timezone normalization is done natively by the host
// (TZ=UTC before Chromium init), not here, so Date and Intl agree by construction.
//
// Injected at document-start into every WEB frame (never a cyberdesk:// UI frame),
// so it runs before any page script. The seed placeholder on the SESSION_SEED line
// below is replaced by the host with a fresh random per-BROWSER-SESSION seed (hex);
// a new launch => a new seed => a different fingerprint (cross-session unlinkable),
// while within one launch the seed is fixed (stable readback, no breakage/flicker).
//
// Mechanism (Brave-style farbling, reimplemented in document-start JS since a CEF
// embedder cannot patch Blink/C++):
//   (a) readback vectors (canvas, WebGL, audio, client rects, text metrics) get
//       DETERMINISTIC per-(session, first-party origin) noise — invisible to the
//       user, but enough to change the fingerprint hash;
//   (b) stable high-entropy attributes (hardwareConcurrency, deviceMemory) are
//       clamped to common buckets, and the explicit local-font enumeration API is
//       neutralized.
//
// Determinism is the crux of "stable within a session": every farble is a PURE
// FUNCTION of (origin key, input), re-seeded per call and walked in a fixed order,
// so repeated reads in one session are byte-identical (a site cannot detect the
// noise by reading twice, and nothing flickers), yet a fresh session's different
// seed yields a different — hence unlinkable — result.

(function () {
  "use strict";

  var SESSION_SEED = "__CYBERDESK_FP_SEED__";

  // The page global. Referenced explicitly (and every DOM constructor is looked up
  // via `W.` and guarded) so a missing global degrades to a no-op instead of
  // throwing and aborting the rest of the hardening — and so the exact same file
  // is exercisable under a headless (Node vm) mock. Built-ins (Math, Object,
  // Function, Promise, WeakSet, Proxy, Array, typed arrays, isFinite) are always
  // present and used bare.
  var W = (typeof window !== "undefined") ? window
        : (typeof self !== "undefined") ? self
        : (typeof globalThis !== "undefined") ? globalThis : this;

  // ---- deterministic primitives ---------------------------------------------

  // FNV-1a (32-bit): mixes strings into the seed. Not cryptographic — the secret
  // is SESSION_SEED; this only decorrelates origins/tags cheaply.
  function fnv1a(str) {
    var h = 0x811c9dc5;
    for (var i = 0; i < str.length; i++) {
      h ^= str.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
    return h >>> 0;
  }

  // Mulberry32: a tiny, fast, well-distributed PRNG. Re-seeding it from the same
  // value reproduces the same stream — the property that makes readback stable.
  function mulberry32(a) {
    return function () {
      a |= 0; a = (a + 0x6d2b79f5) | 0;
      var t = Math.imul(a ^ (a >>> 15), 1 | a);
      t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
      return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
    };
  }

  // First-party origin — keys the noise per top-level site. A tracker embedded as
  // a third-party iframe on two DIFFERENT first parties therefore reads DIFFERENT
  // noise on each, so it cannot correlate the two visits by fingerprint. For a
  // cross-origin iframe (window.top unreadable) we recover the top origin from
  // location.ancestorOrigins (Blink exposes the full chain even cross-origin).
  function firstPartyOrigin() {
    try {
      if (W.top === W) return String(W.location.origin);
      return String(W.top.location.origin); // same-origin ancestors: readable
    } catch (e) {
      try {
        var ao = W.location.ancestorOrigins;
        if (ao && ao.length) return String(ao[ao.length - 1]);
      } catch (e2) { /* fall through */ }
      try { return String(W.location.origin); } catch (e3) { return "null"; }
    }
  }

  var ORIGIN_KEY = fnv1a(SESSION_SEED + "|" + firstPartyOrigin());

  // Per-vector seed: independent noise streams that are all stable within a session.
  function vseed(tag) { return (ORIGIN_KEY ^ fnv1a(tag)) | 0; }

  // Stable per-value scalar jitter (for metrics: rect edges, text width). Keyed on
  // the value itself, so the SAME measured value always perturbs the SAME way —
  // stable within a session, and (because ORIGIN_KEY changes) different across
  // sessions. Magnitude is a small RELATIVE fraction: sub-visual for layout, but it
  // moves the low significant digits that a full-precision fingerprint hashes.
  function jitter(v, tag, rel) {
    if (typeof v !== "number" || !isFinite(v) || v === 0) return v;
    var r = mulberry32((ORIGIN_KEY ^ fnv1a(tag) ^ fnv1a(String(v))) | 0)();
    var mag = Math.abs(v) < 1 ? 1 : Math.abs(v);
    return v + (r - 0.5) * rel * mag;
  }

  // Replace a method non-enumerably (methods stay writable/configurable in Blink).
  function def(obj, name, value) {
    try {
      Object.defineProperty(obj, name, {
        value: value, writable: true, enumerable: false, configurable: true
      });
      return true;
    } catch (e) { return false; }
  }

  // ---- pixel farbling --------------------------------------------------------

  // RGBA-aware: nudge ~4.7% of pixels by ±1 in ONE of R/G/B (never alpha, never
  // more than 1/255) — invisible, but it changes the serialized bytes and thus the
  // hash. Re-seeded per call + walked in order => identical output on repeat reads.
  function farbleRGBA(u8, seed) {
    var rnd = mulberry32(seed | 0);
    var n = u8.length;
    for (var i = 0; i + 3 < n; i += 4) {
      var r = rnd();
      if (r < 0.047) {
        var pick = (r * 100000) | 0;
        var c = pick % 3;                 // R, G or B
        var d = (pick & 1) ? 1 : -1;
        var v = u8[i + c] + d;
        u8[i + c] = v < 0 ? 0 : (v > 255 ? 255 : v);
      }
    }
  }

  // Generic byte farble (WebGL readPixels: format/type not assumed). Nudges a small
  // fraction of bytes by ±1.
  function farbleU8(u8, seed) {
    var rnd = mulberry32(seed | 0);
    for (var i = 0; i < u8.length; i++) {
      if (rnd() < 0.032) {
        var d = rnd() < 0.5 ? -1 : 1;
        var v = u8[i] + d;
        u8[i] = v < 0 ? 0 : (v > 255 ? 255 : v);
      }
    }
  }

  // ============ Canvas 2D readback ===========================================
  (function canvas() {
    var HCE = W.HTMLCanvasElement, C2D = W.CanvasRenderingContext2D;
    var _getImageData = C2D && C2D.prototype.getImageData;
    var _toDataURL = HCE && HCE.prototype.toDataURL;
    var _toBlob = HCE && HCE.prototype.toBlob;
    if (!C2D || !_getImageData) return;
    var SEED = vseed("canvas");

    // getImageData IS readback — farble the returned pixels in place.
    def(C2D.prototype, "getImageData", function () {
      var img = _getImageData.apply(this, arguments);
      try { farbleRGBA(img.data, SEED); } catch (e) {}
      return img;
    });

    // toDataURL / toBlob serialize the backing store directly (not via the hooked
    // getImageData), so farble a private COPY and serialize that — never the live
    // canvas (no visible change). drawImage works from a 2D OR a WebGL source, so
    // this covers WebGL canvases too. The original getImageData is used internally
    // to avoid double-farbling.
    function farbledCopy(canvasEl) {
      try {
        var w = canvasEl.width, h = canvasEl.height;
        if (!w || !h || !W.document) return null;
        var tmp = W.document.createElement("canvas");
        tmp.width = w; tmp.height = h;
        var ctx = tmp.getContext("2d");
        if (!ctx) return null;
        ctx.drawImage(canvasEl, 0, 0);
        var img = _getImageData.call(ctx, 0, 0, w, h);
        farbleRGBA(img.data, SEED);
        ctx.putImageData(img, 0, 0);
        return tmp;
      } catch (e) { return null; }
    }
    if (_toDataURL) def(HCE.prototype, "toDataURL", function () {
      var snap = farbledCopy(this);
      return _toDataURL.apply(snap || this, arguments);
    });
    if (_toBlob) def(HCE.prototype, "toBlob", function () {
      var snap = farbledCopy(this);
      return _toBlob.apply(snap || this, arguments);
    });

    // OffscreenCanvas (also used for fingerprinting in the document context).
    var OSC = W.OffscreenCanvas;
    var OSC2D = W.OffscreenCanvasRenderingContext2D;
    var _oscGID = OSC2D && OSC2D.prototype.getImageData;
    if (OSC2D && _oscGID) def(OSC2D.prototype, "getImageData", function () {
      var img = _oscGID.apply(this, arguments);
      try { farbleRGBA(img.data, SEED); } catch (e) {}
      return img;
    });
    var _convert = OSC && OSC.prototype.convertToBlob;
    if (OSC && _convert) def(OSC.prototype, "convertToBlob", function () {
      try {
        var w = this.width, h = this.height;
        if (w && h) {
          var tmp = new OSC(w, h);
          var ctx = tmp.getContext("2d");
          if (ctx) {
            ctx.drawImage(this, 0, 0);
            var img = (_oscGID || _getImageData).call(ctx, 0, 0, w, h);
            farbleRGBA(img.data, SEED);
            ctx.putImageData(img, 0, 0);
            return _convert.apply(tmp, arguments);
          }
        }
      } catch (e) {}
      return _convert.apply(this, arguments);
    });
  })();

  // ============ WebGL readback + parameter standardization ===================
  (function webgl() {
    var SEED = vseed("webgl");
    var VENDOR = 0x9245, RENDERER = 0x9246; // UNMASKED_*_WEBGL (debug_renderer_info)
    // A common, Windows-COHERENT ANGLE/D3D11 Intel string (the single most common
    // desktop bucket). Standardizing the two unmasked strings collapses the GPU
    // entropy without contradicting the real OS: ANGLE+D3D11 is a Windows path, so
    // it agrees with the (untouched, real) Windows UA/platform. Every other
    // getParameter enum is passed straight through — we do not touch capabilities.
    var STD_VENDOR = "Google Inc. (Intel)";
    var STD_RENDERER =
      "ANGLE (Intel, Intel(R) UHD Graphics 630 Direct3D11 vs_5_0 ps_5_0, D3D11)";

    function patch(GL) {
      if (!GL || !GL.prototype) return;
      var _getParameter = GL.prototype.getParameter;
      var _readPixels = GL.prototype.readPixels;
      if (_getParameter) def(GL.prototype, "getParameter", function (p) {
        if (p === VENDOR) return STD_VENDOR;
        if (p === RENDERER) return STD_RENDERER;
        return _getParameter.apply(this, arguments);
      });
      if (_readPixels) def(GL.prototype, "readPixels", function () {
        var ret = _readPixels.apply(this, arguments);
        try {
          var px = arguments[6]; // pixels: ArrayBufferView
          if (px && (px instanceof Uint8Array || px instanceof Uint8ClampedArray)) {
            farbleU8(px, SEED);
          }
        } catch (e) {}
        return ret;
      });
    }
    patch(W.WebGLRenderingContext);
    patch(W.WebGL2RenderingContext);
  })();

  // ============ AudioContext readback ========================================
  (function audio() {
    // Inaudible, deterministic perturbation. Additive with a small floor so silent
    // samples (and dB-scale frequency bins) are still moved; magnitude ~1e-4
    // relative is far below the audible threshold but shifts the significant digits
    // a full-precision audio fingerprint sums.
    function farbleFloat(arr, seed) {
      var rnd = mulberry32(seed | 0);
      for (var i = 0; i < arr.length; i++) {
        var s = arr[i];
        arr[i] = s + (rnd() - 0.5) * 1e-4 * (Math.abs(s) + 1e-3);
      }
    }
    function farbleByte(arr, seed) {
      var rnd = mulberry32(seed | 0);
      for (var i = 0; i < arr.length; i++) {
        if (rnd() < 0.02) {
          var d = rnd() < 0.5 ? -1 : 1;
          var v = arr[i] + d;
          arr[i] = v < 0 ? 0 : (v > 255 ? 255 : v);
        }
      }
    }

    var AB = W.AudioBuffer;
    if (AB && AB.prototype) {
      var _getChannelData = AB.prototype.getChannelData;
      var _copyFromChannel = AB.prototype.copyFromChannel;
      // getChannelData hands back the LIVE backing Float32Array. Farble each such
      // array exactly ONCE (WeakSet) so repeated reads are stable AND playback is
      // never re-perturbed; the change is inaudible either way.
      var seen = new WeakSet();
      if (_getChannelData) def(AB.prototype, "getChannelData", function (ch) {
        var data = _getChannelData.apply(this, arguments);
        try {
          if (data && !seen.has(data)) {
            seen.add(data);
            farbleFloat(data, vseed("audio") ^ ((ch | 0) + 1));
          }
        } catch (e) {}
        return data;
      });
      // copyFromChannel copies into a caller buffer (source untouched → playback
      // safe); farble the copy deterministically per call.
      if (_copyFromChannel) def(AB.prototype, "copyFromChannel", function (dest, ch) {
        _copyFromChannel.apply(this, arguments);
        try { farbleFloat(dest, vseed("audio") ^ ((ch | 0) + 1)); } catch (e) {}
      });
    }

    var AN = W.AnalyserNode;
    if (AN && AN.prototype) {
      ["getFloatFrequencyData", "getFloatTimeDomainData"].forEach(function (m) {
        var orig = AN.prototype[m];
        if (orig) def(AN.prototype, m, function (arr) {
          orig.apply(this, arguments);
          try { farbleFloat(arr, vseed("audio:" + m)); } catch (e) {}
        });
      });
      ["getByteFrequencyData", "getByteTimeDomainData"].forEach(function (m) {
        var orig = AN.prototype[m];
        if (orig) def(AN.prototype, m, function (arr) {
          orig.apply(this, arguments);
          try { farbleByte(arr, vseed("audio:" + m)); } catch (e) {}
        });
      });
    }
  })();

  // ============ Client rects + text metrics ==================================
  (function metrics() {
    var DOMRectCtor = W.DOMRect;
    function jitterRect(r) {
      if (!r || !DOMRectCtor) return r;
      try {
        return new DOMRectCtor(
          jitter(r.x, "rect:x", 3e-3), jitter(r.y, "rect:y", 3e-3),
          jitter(r.width, "rect:w", 3e-3), jitter(r.height, "rect:h", 3e-3)
        );
      } catch (e) { return r; }
    }
    // Element and Range both expose getBoundingClientRect / getClientRects. The
    // jitter is keyed on the value, so a single-rect element's bounding rect and
    // its getClientRects()[0] receive the SAME perturbation — they stay mutually
    // consistent (no self-contradiction a script could detect).
    [W.Element, W.Range].forEach(function (Ctor) {
      if (!Ctor || !Ctor.prototype) return;
      var _bcr = Ctor.prototype.getBoundingClientRect;
      if (_bcr) def(Ctor.prototype, "getBoundingClientRect", function () {
        return jitterRect(_bcr.apply(this, arguments));
      });
      var _cr = Ctor.prototype.getClientRects;
      if (_cr) def(Ctor.prototype, "getClientRects", function () {
        var list = _cr.apply(this, arguments);
        try {
          var out = [];
          for (var i = 0; i < list.length; i++) out.push(jitterRect(list[i]));
          out.item = function (n) { return this[n] || null; };
          return out;
        } catch (e) { return list; }
      });
    });

    // measureText width is THE text/font-metric fingerprint. We wrap the returned
    // TextMetrics in a Proxy that jitters every numeric field (width plus the
    // bounding-box metrics) while preserving the prototype (instanceof still holds).
    var C2D = W.CanvasRenderingContext2D;
    var _measureText = C2D && C2D.prototype.measureText;
    if (_measureText) def(C2D.prototype, "measureText", function () {
      var m = _measureText.apply(this, arguments);
      try {
        return new Proxy(m, {
          get: function (t, k) {
            var v = t[k];
            if (typeof v === "number") return jitter(v, "text:" + String(k), 3e-3);
            return (typeof v === "function") ? v.bind(t) : v;
          }
        });
      } catch (e) { return m; }
    });
  })();

  // ============ Entropy reduction on stable attributes =======================
  (function navAttrs() {
    var Nav = W.Navigator, nav = W.navigator;
    // hardwareConcurrency → nearest common bucket AT OR BELOW the real value (never
    // over-report cores; floor 2). Collapses many machines onto {2,4,8,16}.
    try {
      if (Nav && Nav.prototype) {
        var real = (nav && nav.hardwareConcurrency) || 4;
        var buckets = [2, 4, 8, 16], cores = 2;
        for (var i = 0; i < buckets.length; i++) if (buckets[i] <= real) cores = buckets[i];
        Object.defineProperty(Nav.prototype, "hardwareConcurrency", {
          get: function () { return cores; }, configurable: true, enumerable: true
        });
      }
    } catch (e) {}
    // deviceMemory is already spec-bucketed {0.25..8}; collapse further to {4,8}
    // (the common desktop buckets) so it stops distinguishing exact RAM tiers.
    try {
      if (Nav && Nav.prototype && nav && ("deviceMemory" in nav)) {
        var mem = ((nav.deviceMemory || 8) >= 4) ? 8 : 4;
        Object.defineProperty(Nav.prototype, "deviceMemory", {
          get: function () { return mem; }, configurable: true, enumerable: true
        });
      }
    } catch (e) {}
  })();

  // ============ Font enumeration =============================================
  (function fonts() {
    // Neutralize the explicit Local Font Access enumeration API (a high-signal,
    // cleanly removable vector): report no locally-installed fonts. (Width-based
    // font PROBING via measureText/rects is only partially mitigated by the metric
    // jitter above — it breaks the LINKABILITY of the metric fingerprint across
    // sessions, but does not fully hide which fonts exist; complete font-set
    // standardization needs the Chromium font backend, deferred. See D-0039.)
    function noFonts() { return Promise.resolve([]); }
    try { if (W.queryLocalFonts) def(W, "queryLocalFonts", noFonts); } catch (e) {}
    try {
      if (W.Navigator && W.Navigator.prototype && ("queryLocalFonts" in W.Navigator.prototype)) {
        def(W.Navigator.prototype, "queryLocalFonts", noFonts);
      }
    } catch (e) {}
  })();

})();
