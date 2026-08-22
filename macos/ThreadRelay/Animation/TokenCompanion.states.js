// Mochi artwork notes.
//
// The dashboard uses the native SwiftUI implementation in
// AIGlassUI/TokenCompanionAnimator.swift. This small state file remains a
// design source of truth for previews and future asset export: one white
// silhouette and a restrained set of meaningful status states.

window.PREVIEW_CONFIG = {
  title: "Mochi",
  subtitle: "A soft dango companion",
  viewBox: "0 0 76 58",
  size: { width: 76, height: 58 },
  exportName: "TokenCompanionAnimator",
};

const body = {
  tag: "path",
  // Directly mapped from the CodePen dango: border-radius 250px 250px 100px 100px.
  d: "M34.3 9.4 L41.7 9.4 C56.8 9.4 69 21.7 69 36.9 C69 43.2 63.9 48.4 57.6 48.4 L18.4 48.4 C12.1 48.4 7 43.2 7 36.9 C7 21.7 19.2 9.4 34.3 9.4 Z",
  fill: "currentColor",
};

const face = {
  eyeL: { tag: "rect", x: "30.1", y: "19.3", width: "2.0", height: "10.7", rx: "1", fill: "#4E4E4E" },
  eyeR: { tag: "rect", x: "43.9", y: "19.3", width: "2.0", height: "10.7", rx: "1", fill: "#4E4E4E" },
  cheekL: { tag: "ellipse", cx: "24.2", cy: "29.1", rx: "4.1", ry: "4.1", fill: "#D8A5B2", opacity: "0.55" },
  cheekR: { tag: "ellipse", cx: "51.8", cy: "29.1", rx: "4.1", ry: "4.1", fill: "#D8A5B2", opacity: "0.55" },
};

const blink = {
  kind: "blink",
  selectorAll: "#path-eyeL, #path-eyeR",
  period: 4.6,
  blinkDuration: 0.16,
  minScale: 0.12,
};

const breathe = {
  kind: "compound",
  parts: [
    { kind: "breathe-y", duration: 3.8, amplitude: 0.018, origin: "38px 49px" },
    blink,
  ],
};

window.STATES_DATA = {
  idle: { paths: { body, ...face }, idle: breathe },
  working: { paths: { body, ...face }, idle: { kind: "bob", duration: 1.45, amplitude: 0.02, origin: "38px 49px" } },
  waiting: { paths: { body, ...face }, idle: { kind: "breathe-y", duration: 2.1, amplitude: 0.014, origin: "38px 49px" } },
  success: { paths: { body, ...face }, idle: { kind: "bob", duration: 0.86, amplitude: 0.06, origin: "38px 49px" } },
  error: { paths: { body, ...face }, idle: { kind: "shake", duration: 0.86, amplitude: 0.02, origin: "38px 49px" } },
  disconnected: { paths: { body, ...face }, idle: { kind: "breathe-y", duration: 4.8, amplitude: 0.008, origin: "38px 49px" } },
};
