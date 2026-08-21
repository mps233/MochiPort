// Original ThreadRelay companion artwork for svg-character-animator.
// The repository's Swift exporter turns these shared SVG-like paths into a
// self-contained SwiftUI view with idle motion and state transitions.

window.PREVIEW_CONFIG = {
  title: "Token Companion",
  subtitle: "Monochrome ThreadRelay token companion",
  viewBox: "0 0 76 58",
  size: { width: 76, height: 58 },
  exportName: "TokenCompanionAnimator",
};

const blink = {
  kind: "blink",
  selectorAll: "#path-eyeL, #path-eyeR",
  period: 4.6,
  blinkDuration: 0.16,
  minScale: 0.12,
};

const idle = {
  kind: "compound",
  parts: [
    { kind: "breathe-y", duration: 3.8, amplitude: 0.018, origin: "38px 51px" },
    blink,
  ],
};

window.STATES_DATA = {
  idle: {
    paths: {
      body: {
        tag: "path",
        d: "M13 32 C13 22 18 15 27 13 C34 11.5 42 11.5 49 13 C58 15 63 22 63 32 C63 42 57 49 48 51 C41 52.5 35 52.5 28 51 C19 49 13 42 13 32 Z",
        fill: "#F0F0F0",
        stroke: "#FFFFFF",
        strokeWidth: "2.6",
      },
      earL: {
        tag: "path",
        d: "M22 16 C18 13 18 8 20 6 C22 4 26 8 29 13 Z",
        fill: "#E4E4E4",
        stroke: "#FFFFFF",
        strokeWidth: "2.4",
      },
      earR: {
        tag: "path",
        d: "M47 13 C50 8 54 4 56 6 C58 8 58 13 54 16 Z",
        fill: "#E4E4E4",
        stroke: "#FFFFFF",
        strokeWidth: "2.4",
      },
      eyeL: { tag: "ellipse", cx: "28.5", cy: "30", rx: "2.2", ry: "3", fill: "#FFFFFF" },
      eyeR: { tag: "ellipse", cx: "47.5", cy: "30", rx: "2.2", ry: "3", fill: "#FFFFFF" },
      cheekL: { tag: "ellipse", cx: "23.5", cy: "37", rx: "3", ry: "1.4", fill: "#C9C9C9" },
      cheekR: { tag: "ellipse", cx: "52.5", cy: "37", rx: "3", ry: "1.4", fill: "#C9C9C9" },
      mouth: {
        tag: "path",
        d: "M34 37 C36 39.5 40 39.5 42 37",
        stroke: "#FFFFFF",
        strokeWidth: "2.3",
        strokeLinecap: "round",
        fill: "none",
      },
      footL: { tag: "path", d: "M25 48 C24 52 25 54 29 54 C32 54 33 52 32 49 Z", fill: "#D7D7D7", stroke: "#FFFFFF", strokeWidth: "2.2" },
      footR: { tag: "path", d: "M44 49 C43 52 44 54 47 54 C51 54 52 52 51 48 Z", fill: "#D7D7D7", stroke: "#FFFFFF", strokeWidth: "2.2" },
      armL: { tag: "path", d: "M14 32 C10 33 9 36 11 38", stroke: "#FFFFFF", strokeWidth: "2.3", strokeLinecap: "round", fill: "none" },
      armR: { tag: "path", d: "M62 32 C66 33 67 36 65 38", stroke: "#FFFFFF", strokeWidth: "2.3", strokeLinecap: "round", fill: "none" },
    },
    idle,
  },
  happy: {
    paths: {
      body: {
        tag: "path",
        d: "M13 31 C13 21 18 14 27 12 C34 10.5 42 10.5 49 12 C58 14 63 21 63 31 C63 41 57 48 48 50 C41 51.5 35 51.5 28 50 C19 48 13 41 13 31 Z",
        fill: "#F0F0F0",
        stroke: "#FFFFFF",
        strokeWidth: "2.6",
      },
      earL: {
        tag: "path",
        d: "M22 15 C18 12 18 7 20 5 C22 3 26 7 29 12 Z",
        fill: "#E4E4E4",
        stroke: "#FFFFFF",
        strokeWidth: "2.4",
      },
      earR: {
        tag: "path",
        d: "M47 12 C50 7 54 3 56 5 C58 7 58 12 54 15 Z",
        fill: "#E4E4E4",
        stroke: "#FFFFFF",
        strokeWidth: "2.4",
      },
      eyeL: { tag: "ellipse", cx: "28.5", cy: "29", rx: "2.2", ry: "1.0", fill: "#FFFFFF" },
      eyeR: { tag: "ellipse", cx: "47.5", cy: "29", rx: "2.2", ry: "1.0", fill: "#FFFFFF" },
      cheekL: { tag: "ellipse", cx: "23.5", cy: "36", rx: "3.2", ry: "1.6", fill: "#C9C9C9" },
      cheekR: { tag: "ellipse", cx: "52.5", cy: "36", rx: "3.2", ry: "1.6", fill: "#C9C9C9" },
      mouth: {
        tag: "path",
        d: "M33 36 C35.5 41 40.5 41 43 36",
        stroke: "#FFFFFF",
        strokeWidth: "2.3",
        strokeLinecap: "round",
        fill: "none",
      },
      footL: { tag: "path", d: "M25 47 C24 51 25 53 29 53 C32 53 33 51 32 48 Z", fill: "#D7D7D7", stroke: "#FFFFFF", strokeWidth: "2.2" },
      footR: { tag: "path", d: "M44 48 C43 51 44 53 47 53 C51 53 52 51 51 47 Z", fill: "#D7D7D7", stroke: "#FFFFFF", strokeWidth: "2.2" },
      armL: { tag: "path", d: "M14 31 C10 30 8 32 10 35", stroke: "#FFFFFF", strokeWidth: "2.3", strokeLinecap: "round", fill: "none" },
      armR: { tag: "path", d: "M62 31 C66 30 68 32 66 35", stroke: "#FFFFFF", strokeWidth: "2.3", strokeLinecap: "round", fill: "none" },
    },
    idle: {
      kind: "compound",
      parts: [
        { kind: "bob", duration: 1.5, amplitude: 1.5, origin: "38px 51px" },
        blink,
      ],
    },
  },
};
