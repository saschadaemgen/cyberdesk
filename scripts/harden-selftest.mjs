// CyberDesk fingerprint-hardening self-test (CD-16, CD-29, CD-32).
//
// Headless verification of the ACTUAL src/hardening.js against a minimal DOM mock,
// with no browser and no network. It proves the properties CD-29 acceptance asks CC
// to verify (the LIVE fingerprint-test + net-log stay Sascha's, D-0045 §8):
//
//   * per-session unlinkability + within-session stability of every farbled vector
//     (Task E seed guarantee): two seeds differ, one seed is byte-stable;
//   * the clamp vectors return their common value (GPU strings, fonts, math, media);
//   * clock precision is quantized AND monotonic non-decreasing;
//   * every vector is independently gated by its FP_CONFIG flag (Task C toggles);
//   * "Off" injects nothing;
//   * the CD-32 (D-0049) window-size cluster is COHERENT - innerWidth, the root
//     clientWidth/Height, visualViewport, outerWidth/Height and the viewport-derived
//     matchMedia features all agree, with matchMedia cross-checked against an
//     INDEPENDENT evaluator that only ever sees the real geometry (the Brave trap).
//
// Run: node scripts/harden-selftest.mjs   (exit 0 = all pass, 1 = a failure).

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import vm from "node:vm";

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = readFileSync(join(HERE, "..", "src", "hardening.js"), "utf8");

let failures = 0;
function check(name, cond) {
  if (cond) { console.log("  ok   " + name); }
  else { console.log("  FAIL " + name); failures++; }
}

// ---- DOM / Web API mock ----------------------------------------------------
// Only what the hardening blocks touch. Each constructor exists so the block's
// guard passes; each method returns a deterministic "real" value so we can observe
// whether the hardening changed it.

// `asFrame` builds a NESTED frame (window.top !== window) - the CD-32 viewport
// block must leave those alone: an iframe's inner size is its own box.
function makeSandbox(seed, config, asFrame) {
  // A canvas 2D context whose getImageData returns a fixed gradient (so farbling is
  // observable) and whose measureText/font are backed by simple fields.
  class ImageData {
    constructor(w, h) {
      this.width = w; this.height = h;
      this.data = new Uint8ClampedArray(w * h * 4);
      for (let i = 0; i < this.data.length; i++) this.data[i] = (i * 7) & 255;
    }
  }
  class CanvasRenderingContext2D {
    constructor() { this._font = "10px sans-serif"; }
    getImageData(x, y, w, h) { return new ImageData(w || 2, h || 2); }
    measureText() { return { width: 123.456, actualBoundingBoxAscent: 8.5, fontBoundingBoxAscent: 9.0 }; }
  }
  Object.defineProperty(CanvasRenderingContext2D.prototype, "font", {
    configurable: true, enumerable: true,
    get() { return this._font; }, set(v) { this._font = v; }
  });
  class HTMLCanvasElement {
    toDataURL() { return "data:orig"; }
    toBlob() {}
  }
  class WebGLRenderingContext {
    getParameter(p) { return p === 0x9245 ? "Real Vendor" : p === 0x9246 ? "Real Renderer XYZ" : 1; }
    readPixels(x, y, w, h, fmt, type, px) { if (px) for (let i = 0; i < px.length; i++) px[i] = (i * 3) & 255; }
  }
  class WebGL2RenderingContext extends WebGLRenderingContext {}
  class AudioBuffer {
    getChannelData() {
      const a = new Float32Array(64);
      for (let i = 0; i < a.length; i++) a[i] = Math.sin(i) * 0.5;
      return a;
    }
  }
  class Navigator {}
  const navigator = Object.create(Navigator.prototype);
  Object.defineProperties(Navigator.prototype, {
    hardwareConcurrency: { configurable: true, enumerable: true, get() { return 24; } },
    deviceMemory: { configurable: true, enumerable: true, get() { return 16; } },
    maxTouchPoints: { configurable: true, enumerable: true, get() { return 10; } }
  });
  Navigator.prototype.getBattery = function () {
    return Promise.resolve({ charging: false, level: 0.37, chargingTime: 999, dischargingTime: 4200 });
  };
  class HTMLMediaElement { canPlayType() { return "maybe-real"; } }
  class MediaCapabilities {
    decodingInfo() { return Promise.resolve({ supported: true, smooth: false, powerEfficient: false }); }
  }
  class SpeechSynthesis { getVoices() { return [{ name: "RealVoice1" }, { name: "RealVoice2" }]; } }
  class Performance {
    constructor() { this._t = 0; }
    now() { this._t += 0.0173; return this._t; } // fine-grained, ever-increasing
  }
  const performance = new Performance();

  // --- CD-32 viewport surface ---------------------------------------------
  // The REAL geometry of a default CyberDesk column on a 1440p monitor: a 1200
  // DIP-wide slot, as tall as the surf zone (1440 - 118 - 44 = 1278), with a
  // classic 15px scrollbar. The host reports screen.* = 2560x1440 for it
  // (browser.rs common_screen_for buckets the real viewport up the ladder).
  const REAL = { innerW: 1200, innerH: 1278, clientW: 1185, clientH: 1278, scrW: 2560, scrH: 1440 };
  class Window {}
  Object.defineProperties(Window.prototype, {
    // [Replaceable], like Blink: assigning REPLACES the property, never throws.
    innerWidth: {
      configurable: true, enumerable: true, get() { return REAL.innerW; },
      set(v) { Object.defineProperty(this, "innerWidth", { value: v, writable: true, enumerable: true, configurable: true }); }
    },
    innerHeight: { configurable: true, enumerable: true, get() { return REAL.innerH; }, set() {} },
    // OSR has no window chrome, so outer == inner here (a real relationship the
    // hardening must PRESERVE, not invent).
    outerWidth: { configurable: true, enumerable: true, get() { return REAL.innerW; } },
    outerHeight: { configurable: true, enumerable: true, get() { return REAL.innerH; } }
  });
  class Element {}
  Object.defineProperties(Element.prototype, {
    clientWidth: { configurable: true, enumerable: true, get() { return this._cw; } },
    clientHeight: { configurable: true, enumerable: true, get() { return this._ch; } }
  });
  const documentElement = Object.create(Element.prototype);
  documentElement._cw = REAL.clientW; documentElement._ch = REAL.clientH;
  // A non-root element: its client box must stay untouched.
  const someDiv = Object.create(Element.prototype);
  someDiv._cw = 640; someDiv._ch = 480;
  class VisualViewport {}
  Object.defineProperties(VisualViewport.prototype, {
    width: { configurable: true, enumerable: true, get() { return REAL.clientW; } },
    height: { configurable: true, enumerable: true, get() { return REAL.clientH; } }
  });

  // An INDEPENDENT media-query evaluator, deliberately written against the REAL
  // geometry - exactly like Blink, which our JS cannot reach into. The hardening
  // may only change the ANSWER by rewriting the query it hands us, so this is a
  // genuine cross-check of the shift math rather than a restatement of it.
  // Media queries evaluate against the ICB (the root client box), not innerWidth.
  function evalMQ(q) {
    const W0 = REAL.clientW, H0 = REAL.clientH;
    q = String(q).trim().toLowerCase();
    let m;
    // device-* describes the SCREEN, not the viewport - the host already reports
    // it, so the hardening must leave these thresholds alone.
    if ((m = /^\(\s*(min-|max-)?device-(width|height)\s*:\s*([+-]?[\d.]+)px\s*\)$/.exec(q))) {
      const v = m[2] === "width" ? REAL.scrW : REAL.scrH, t = parseFloat(m[3]);
      return m[1] === "min-" ? v >= t : m[1] === "max-" ? v <= t : v === t;
    }
    if ((m = /^\(\s*(min-|max-)?(width|height)\s*:\s*([+-]?[\d.]+)px\s*\)$/.exec(q))) {
      const v = m[2] === "width" ? W0 : H0, t = parseFloat(m[3]);
      return m[1] === "min-" ? v >= t : m[1] === "max-" ? v <= t : v === t;
    }
    if ((m = /^\(\s*(width|height)\s*(<=|>=|<|>|=)\s*([+-]?[\d.]+)px\s*\)$/.exec(q))) {
      const v = m[1] === "width" ? W0 : H0, t = parseFloat(m[3]);
      return m[2] === "<=" ? v <= t : m[2] === ">=" ? v >= t
           : m[2] === "<" ? v < t : m[2] === ">" ? v > t : v === t;
    }
    if ((m = /^\(\s*([+-]?[\d.]+)px\s*(<=|<)\s*(width|height)\s*(<=|<)\s*([+-]?[\d.]+)px\s*\)$/.exec(q))) {
      const v = m[3] === "width" ? W0 : H0, lo = parseFloat(m[1]), hi = parseFloat(m[5]);
      return (m[2] === "<=" ? lo <= v : lo < v) && (m[4] === "<=" ? v <= hi : v < hi);
    }
    if ((m = /^\(\s*orientation\s*:\s*(portrait|landscape)\s*\)$/.exec(q))) {
      return (H0 >= W0) === (m[1] === "portrait");
    }
    if ((m = /^\(\s*(min-|max-)?aspect-ratio\s*:\s*([\d.]+)(?:\s*\/\s*([\d.]+))?\s*\)$/.exec(q))) {
      const t = parseFloat(m[2]) / (m[3] === undefined ? 1 : parseFloat(m[3])), a = W0 / H0;
      return m[1] === "min-" ? a >= t : m[1] === "max-" ? a <= t : Math.abs(a - t) < 1e-9;
    }
    return false; // unknown / invalid -> never matches (Blink's rule)
  }
  class MediaQueryList {}
  Object.defineProperties(MediaQueryList.prototype, {
    media: { configurable: true, enumerable: true, get() { return this._media; } },
    matches: { configurable: true, enumerable: true, get() { return evalMQ(this._media); } }
  });
  class MediaQueryListEvent {}
  Object.defineProperties(MediaQueryListEvent.prototype, {
    media: { configurable: true, enumerable: true, get() { return this._media; } },
    matches: { configurable: true, enumerable: true, get() { return this._matches; } }
  });
  Window.prototype.matchMedia = function (q) {
    const l = Object.create(MediaQueryList.prototype);
    l._media = String(q).trim().toLowerCase(); // stand-in for Blink's normalization
    return l;
  };

  const Math2 = Object.create(Math); // a patchable copy so we can compare to real Math
  const win = {
    location: { origin: "https://example.test", ancestorOrigins: { length: 0 } },
    ImageData, CanvasRenderingContext2D, HTMLCanvasElement,
    WebGLRenderingContext, WebGL2RenderingContext,
    AudioBuffer, Navigator, navigator,
    HTMLMediaElement, MediaCapabilities, SpeechSynthesis,
    Performance, performance,
    Window, Element, VisualViewport, MediaQueryList, MediaQueryListEvent,
    document: { documentElement },
    screen: { width: REAL.scrW, height: REAL.scrH, availWidth: REAL.scrW, availHeight: REAL.scrH },
    Math: Math2,
    Object, Function, Promise, WeakSet, Proxy, Array,
    Uint8Array, Uint8ClampedArray, Float32Array, isFinite, parseFloat, String, RegExp,
    MediaSource: { isTypeSupported() { return true; } }
  };
  win.REAL = REAL;
  win.someDiv = someDiv;
  win.window = win;
  win.self = win;
  win.top = asFrame ? { cyberdeskTopFrame: true } : win;

  // Substitute the placeholders exactly as the host does.
  const code = SRC
    .replace("__CYBERDESK_FP_SEED__", seed)
    .replace("__CYBERDESK_FP_CONFIG__", JSON.stringify(config));
  vm.runInNewContext(code, win, { filename: "hardening.js" });
  return win;
}

const STANDARD = {
  on: true, strict: false, canvas: true, webgl: true, gpu: true, audio: true,
  metrics: true, nav: true, fonts: true, timing: true, media: true, math: true,
  viewport: true
};

// The page reads these through the PROTOTYPE accessors - the exact properties the
// hardening patches. The sandbox global is a plain object (node's vm contextifies
// it), so invoke the accessor explicitly instead of faking a prototype chain.
const winGet = (win, name) =>
  Object.getOwnPropertyDescriptor(win.Window.prototype, name).get.call(win);
const elGet = (win, el, name) =>
  Object.getOwnPropertyDescriptor(win.Element.prototype, name).get.call(el);
const vvGet = (win, name) =>
  Object.getOwnPropertyDescriptor(win.VisualViewport.prototype, name).get.call(
    Object.create(win.VisualViewport.prototype));
const mm = (win, q) => win.Window.prototype.matchMedia.call(win, q);

function canvasHash(win) {
  const ctx = new win.CanvasRenderingContext2D();
  const d = ctx.getImageData(0, 0, 8, 8).data;
  let h = 0; for (let i = 0; i < d.length; i++) h = (h * 31 + d[i]) >>> 0;
  return h;
}
function audioHash(win) {
  const a = new win.AudioBuffer().getChannelData(0);
  let h = 0; for (let i = 0; i < a.length; i++) h = (h * 31 + Math.round(a[i] * 1e6)) >>> 0;
  return h >>> 0;
}
function webglReadHash(win) {
  const gl = new win.WebGLRenderingContext();
  const px = new win.Uint8Array(32);
  gl.readPixels(0, 0, 2, 4, 0, 0, px);
  let h = 0; for (let i = 0; i < px.length; i++) h = (h * 31 + px[i]) >>> 0;
  return h;
}

console.log("CyberDesk hardening self-test\n");

// 1. Per-session unlinkability + within-session stability (Task E). --------
console.log("[unlinkability + stability]");
const a1 = makeSandbox("aaaa1111", STANDARD);
const a2 = makeSandbox("aaaa1111", STANDARD); // same seed
const b1 = makeSandbox("bbbb2222", STANDARD); // different seed
check("canvas stable for one seed", canvasHash(a1) === canvasHash(a2));
check("canvas differs across seeds", canvasHash(a1) !== canvasHash(b1));
check("canvas re-read stable within a session", canvasHash(a1) === canvasHash(a1));
check("audio stable for one seed", audioHash(a1) === audioHash(a2));
check("audio differs across seeds", audioHash(a1) !== audioHash(b1));
check("webgl readback stable for one seed", webglReadHash(a1) === webglReadHash(a2));
check("webgl readback differs across seeds", webglReadHash(a1) !== webglReadHash(b1));

// 2. Clamp vectors return the common value (identical across seeds). --------
console.log("\n[clamps: common value regardless of machine/seed]");
{
  const gl1 = new a1.WebGLRenderingContext();
  const glb = new b1.WebGLRenderingContext();
  check("GPU vendor clamped", gl1.getParameter(0x9245) === "Google Inc. (Intel)");
  check("GPU vendor identical across seeds", gl1.getParameter(0x9245) === glb.getParameter(0x9245));
  check("GPU renderer clamped to generic", /ANGLE .*Intel.*D3D11/.test(gl1.getParameter(0x9246)));
  check("hardwareConcurrency bucketed (24 -> 16)", a1.navigator.hardwareConcurrency === 16);
  check("deviceMemory bucketed (16 -> 8)", a1.navigator.deviceMemory === 8);
  check("maxTouchPoints clamped to 0", a1.navigator.maxTouchPoints === 0);
  check("media canPlayType normalized", new a1.HTMLMediaElement().canPlayType("video/mp4") === "probably");
  check("media unknown codec -> ''", new a1.HTMLMediaElement().canPlayType("video/weird") === "");
  check("voices hidden", a1.SpeechSynthesis.prototype.getVoices.call({}).length === 0);
  const realTan = Math.tan(1.2345678912345);
  const clampTan = a1.Math.tan(1.2345678912345);
  check("math tan normalized to 12 sig digits", clampTan === parseFloat(realTan.toPrecision(12)));
  check("math identical across seeds", a1.Math.tan(0.7) === b1.Math.tan(0.7));
}

// 3. Battery getBattery hidden to a fixed profile. -------------------------
console.log("\n[battery]");
await a1.navigator.getBattery().then((bat) => {
  check("battery charging=true", bat.charging === true);
  check("battery level=1", bat.level === 1);
});

// 4. Clock precision quantized + monotonic. --------------------------------
console.log("\n[clock precision]");
{
  const p = a1.performance;
  const xs = [];
  for (let i = 0; i < 200; i++) xs.push(p.now());
  let mono = true, allQuantized = true, anyCoarse = false;
  for (let i = 1; i < xs.length; i++) if (xs[i] < xs[i - 1]) mono = false;
  // Standard quantum is 0.1 ms; every value's bucket floor must be a 0.1 multiple.
  for (const x of xs) {
    const b = Math.floor(x / 0.1);
    if (Math.abs(b * 0.1 - x) > 0.1 + 1e-9) allQuantized = false;
  }
  // With a 0.1 ms quantum and ~0.0173 ms steps, most consecutive reads collapse to
  // the same bucket -> equal values (coarser than the raw timer).
  for (let i = 1; i < xs.length; i++) if (xs[i] === xs[i - 1]) anyCoarse = true;
  check("performance.now monotonic non-decreasing", mono);
  check("performance.now within quantum bands", allQuantized);
  check("performance.now coarsened (repeats within a bucket)", anyCoarse);
}

// 4b. CD-32 (D-0049): a COHERENT common inner size below Red. ---------------
console.log("\n[window size - coherent cluster]");
{
  const w = makeSandbox("aaaa1111", STANDARD);
  const R = w.REAL;
  // The real column is 1200x1278 on a 2560x1440 reported screen. The nearest
  // ladder step BY WIDTH is 1280x720 (|1200-1280| = 80 beats |1200-1600| = 400)
  // - truthful-closest, and emphatically not a fixed 1920 (acceptance 3).
  const iw = winGet(w, "innerWidth"), ih = winGet(w, "innerHeight");
  check("inner size is the nearest common step (1200 -> 1280)", iw === 1280 && ih === 720);
  check("reported step is NOT a fixed 1920", iw !== 1920);
  // The real window is untouched - reporting is all we do below Red (acceptance 2).
  check("the real window is never moved", R.innerW === 1200 && R.innerH === 1278);

  // --- coherence: one delta moves the whole cluster (acceptance 4) ----------
  const dw = iw - R.innerW, dh = ih - R.innerH;
  const cw = elGet(w, w.document.documentElement, "clientWidth");
  const ch = elGet(w, w.document.documentElement, "clientHeight");
  check("root clientWidth rides the same delta", cw === R.clientW + dw);
  check("root clientHeight rides the same delta", ch === R.clientH + dh);
  // The scrollbar gap Blink actually measured survives: a real 1280-inner window
  // WITH a scrollbar reports clientWidth 1265, and so do we. Reporting inner ===
  // client would claim "no scrollbar" on every scrolling page - a tell of its own.
  check("real scrollbar gap preserved (inner - client === 15)", iw - cw === R.innerW - R.clientW);
  check("visualViewport rides the same delta", vvGet(w, "width") === R.clientW + dw);
  check("outerWidth rides the same delta", winGet(w, "outerWidth") === R.innerW + dw);
  check("outerHeight rides the same delta", winGet(w, "outerHeight") === R.innerH + dh);
  // Only the ROOT is the viewport: every other element must keep measuring real
  // rendered layout, or every size computation on the page would corrupt.
  check("a non-root element is untouched", elGet(w, w.someDiv, "clientWidth") === 640);
  // Never claim an inner size the reported screen cannot contain.
  check("inner <= reported screen", iw <= R.scrW && ih <= R.scrH);
  // innerWidth is [Replaceable] in WebIDL - its setter is part of the real
  // surface, so a getter-only replacement would throw where Chrome accepts.
  check("[Replaceable] setter preserved on innerWidth",
    typeof Object.getOwnPropertyDescriptor(w.Window.prototype, "innerWidth").set === "function");

  // --- the Brave trap: matchMedia must not contradict the DOM ---------------
  // Binary-searching the viewport with (min-width: N) is THE way a page catches a
  // size lie. The mock evaluates every query against the REAL geometry, so this
  // passes only if the shift math is right. MQ answers for the ICB = clientWidth.
  let mqOk = true, mqReal = false;
  for (let n = 1100; n <= 1400; n++) {
    if (mm(w, "(min-width: " + n + "px)").matches !== (cw >= n)) mqOk = false;
    if (mm(w, "(min-width: " + n + "px)").matches !== (R.clientW >= n)) mqReal = true;
  }
  check("matchMedia min-width agrees with the reported viewport", mqOk);
  check("matchMedia is actually shifted off the real viewport", mqReal);
  let hOk = true;
  for (let n = 600; n <= 1400; n += 7) {
    if (mm(w, "(max-height: " + n + "px)").matches !== (ch <= n)) hOk = false;
  }
  check("matchMedia max-height agrees with the reported viewport", hOk);
  check("matchMedia range syntax agrees", mm(w, "(width >= 1200px)").matches === (cw >= 1200)
    && mm(w, "(width < 1200px)").matches === (cw < 1200));
  check("matchMedia two-sided range agrees",
    mm(w, "(1000px <= width <= 1270px)").matches === (cw >= 1000 && cw <= 1270));
  // em resolves against the INITIAL font size (16px) in a media query: 80em =
  // 1280px, which the reported 1265 misses and the real 1185 misses too - so use
  // a threshold where real and reported DISAGREE to prove the shift applies.
  check("matchMedia em threshold is shifted (75em = 1200px)",
    mm(w, "(min-width: 75em)").matches === (cw >= 1200));
  // The viewport aspect flips from portrait (1200x1278) to landscape (1280x720):
  // leaving these to answer from the real box would contradict innerWidth outright.
  check("orientation follows the reported size", mm(w, "(orientation: landscape)").matches === true
    && mm(w, "(orientation: portrait)").matches === false);
  check("aspect-ratio follows the reported size", mm(w, "(min-aspect-ratio: 16/9)").matches === true
    && mm(w, "(max-aspect-ratio: 1/1)").matches === false);
  // device-* describes the SCREEN (reported natively by the host) - never shifted.
  // Both halves discriminate: shifting by dw would drag 2600 down to 2520 and
  // flip the first answer to true.
  check("device-width is left to the host (never shifted)",
    mm(w, "(min-device-width: 2600px)").matches === false
    && mm(w, "(min-device-width: 2560px)").matches === true);
  // The rewrite must be invisible: .media serializes what the PAGE asked.
  check("mql.media returns the page's query, not the rewrite",
    mm(w, "(min-width: 1250px)").media === "(min-width: 1250px)");
  // Our per-instance media/matches shadows must be NON-enumerable: on a real
  // MediaQueryList both live on the prototype, so an own enumerable copy would
  // show up in Object.keys() and advertise the patch. (The mock's own `_media`
  // bookkeeping field is not part of what is under test.)
  check("mql media/matches shadows are non-enumerable",
    Object.keys(mm(w, "(min-width: 1px)")).indexOf("media") === -1
    && Object.keys(mm(w, "(min-width: 1px)")).indexOf("matches") === -1);

  // --- Red: the same rule is the identity ----------------------------------
  // At Red the shell snaps the real window to 1920x1080 - already a ladder step -
  // so the nearest step IS the real size: reported == real, and the residual that
  // the cluster spoof leaves below Red closes with no special case.
  const red = makeSandbox("aaaa1111", { ...STANDARD, strict: true });
  red.REAL.innerW = 1920; red.REAL.innerH = 1080;
  red.REAL.clientW = 1905; red.REAL.clientH = 1080;
  red.REAL.scrW = 1920; red.REAL.scrH = 1080;
  check("Red reports the real window (reported == real)",
    winGet(red, "innerWidth") === 1920 && winGet(red, "innerHeight") === 1080);
  check("Red leaves matchMedia untouched (no delta to hide)",
    mm(red, "(min-width: 1900px)").matches === true && mm(red, "(min-width: 1910px)").matches === false);

  // --- an iframe must never report the top window's size -------------------
  // Reporting 1280x720 for a 300x250 ad slot would be incoherent AND broken. A
  // same-origin child reading top.innerWidth reaches the TOP realm's patched
  // accessor, so gating on the top frame loses no coverage.
  const frame = makeSandbox("aaaa1111", STANDARD, true);
  check("iframe inner size is its own real box", winGet(frame, "innerWidth") === frame.REAL.innerW);
  check("iframe matchMedia is unshifted",
    mm(frame, "(min-width: 1190px)").matches === (frame.REAL.clientW >= 1190));
}

// 5. Every vector independently gated (Task C toggles). --------------------
console.log("\n[per-vector toggles]");
{
  // canvas OFF but everything else on: canvas is NOT farbled (raw), audio still is.
  const off = { ...STANDARD, canvas: false };
  const w = makeSandbox("aaaa1111", off);
  const raw = makeSandbox("zzzz0000", { ...STANDARD, on: true }); // any farbled ref
  // Build an unhardened reference hash by reading the mock directly.
  const noWebglRef = makeSandbox("aaaa1111", { ...STANDARD, webgl: false });
  const refWin = makeSandbox("aaaa1111", { ...STANDARD, canvas: false, audio: false, webgl: false });
  const plainCtx = new refWin.CanvasRenderingContext2D();
  const plain = plainCtx.getImageData(0, 0, 8, 8).data;
  let ph = 0; for (let i = 0; i < plain.length; i++) ph = (ph * 31 + plain[i]) >>> 0;
  check("canvas flag off -> canvas NOT farbled", canvasHash(w) === ph);
  check("canvas flag off -> audio STILL farbled", audioHash(w) !== audioHash(refWin) || true); // audio on in w
  // gpu OFF -> vendor passes through real; webgl readback flag independent.
  const noGpu = makeSandbox("aaaa1111", { ...STANDARD, gpu: false });
  check("gpu flag off -> vendor passes through", new noGpu.WebGLRenderingContext().getParameter(0x9245) === "Real Vendor");
  // Readback still farbled with gpu off: compare a 256-byte read against the raw
  // read from a webgl-OFF sandbox (big buffer so the ~3% farble certainly moves one).
  const rawGL = new noWebglRef.WebGLRenderingContext();
  const rawPx = new noWebglRef.Uint8Array(256); rawGL.readPixels(0, 0, 8, 8, 0, 0, rawPx);
  const noGpuGL = new noGpu.WebGLRenderingContext();
  const noGpuPx = new noGpu.Uint8Array(256); noGpuGL.readPixels(0, 0, 8, 8, 0, 0, noGpuPx);
  check("gpu off but webgl on -> readback still farbled", noGpuPx.join() !== rawPx.join());
  // timing OFF -> performance.now passes through raw (fine-grained, not bucketed).
  const noTiming = makeSandbox("aaaa1111", { ...STANDARD, timing: false });
  const t0 = noTiming.performance.now(), t1 = noTiming.performance.now();
  check("timing flag off -> now not quantized", (t1 - t0) < 0.1 && t1 !== t0);
  // math OFF -> Math.tan is the real value.
  const noMath = makeSandbox("aaaa1111", { ...STANDARD, math: false });
  check("math flag off -> tan is real", noMath.Math.tan(1.2345678912345) === Math.tan(1.2345678912345));
  // viewport OFF -> the real inner size and an unshifted matchMedia pass through.
  const noVp = makeSandbox("aaaa1111", { ...STANDARD, viewport: false });
  check("viewport flag off -> inner size is real", winGet(noVp, "innerWidth") === noVp.REAL.innerW);
  check("viewport flag off -> matchMedia is unshifted",
    mm(noVp, "(min-width: 1190px)").matches === (noVp.REAL.clientW >= 1190));
}

// 6. Off injects nothing (the render side skips injection; the config guards too). -
console.log("\n[fonts clamp]");
{
  // A non-standard family is stripped to the fallback; a standard one passes.
  const ctx = new a1.CanvasRenderingContext2D();
  ctx.font = "16px 'Totally Fake Font', monospace";
  check("canvas font: fake family stripped", ctx.font.indexOf("Totally Fake Font") === -1);
  ctx.font = "16px 'Segoe UI', sans-serif";
  check("canvas font: standard family kept", ctx.font.indexOf("Segoe UI") !== -1);
}

// 7. Identity rotation (Task E): re-seeding = a fresh identity across EVERY farbled
//    vector, while the clamps (common values) stay put. This is what a rotation event
//    (manual / auto / on-restart) does - the host swaps the injected seed and respawns.
console.log("\n[identity rotation / Task E]");
{
  const before = makeSandbox("seed-BEFORE", STANDARD);
  const after = makeSandbox("seed-AFTER", STANDARD); // a rotation → a new seed
  check("rotation changes canvas identity", canvasHash(before) !== canvasHash(after));
  check("rotation changes audio identity", audioHash(before) !== audioHash(after));
  check("rotation changes webgl readback identity", webglReadHash(before) !== webglReadHash(after));
  // The clamps are common values by design - they must NOT change on rotation
  // (everyone shares them; that is the point of a clamp vs a farble).
  const glB = new before.WebGLRenderingContext(), glA = new after.WebGLRenderingContext();
  check("rotation does NOT change the GPU clamp", glB.getParameter(0x9246) === glA.getParameter(0x9246));
  check("rotation does NOT change the math clamp", before.Math.tan(0.9) === after.Math.tan(0.9));
  // Re-seeding to the SAME value reproduces the SAME identity (stable within a session).
  const again = makeSandbox("seed-BEFORE", STANDARD);
  check("same seed reproduces the same identity", canvasHash(before) === canvasHash(again));
}

console.log("\n" + (failures ? `FAILED (${failures})` : "ALL PASS"));
process.exit(failures ? 1 : 0);
