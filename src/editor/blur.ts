// SnagCraft image effects: real gaussian blur, pixelate, blackout, spotlight
// dimming and highlighter tint — all applied to the IMAGE layer at export
// resolution, so text stays blurred properly instead of looking like a drop
// shadow.
//
// Why not `ctx.filter = "blur(n)"`? WebKitGTK (the WebView SnagCraft ships on
// Linux) has patchy and version-dependent support for canvas filters. StackBlur
// is a deterministic, dependency-free JS convolution that behaves the same on
// every WebEngine, so gaussian blur goes through it.
//
// Geometry contract: every function here works in "device" pixels, i.e. the
// pixel grid of the flattened layer (scene px * export multiplier). Scaling
// scene -> device happens in `toDevice()`.

import { imageDataRGBA } from "stackblur-canvas";

export type BlurMode = "gaussian" | "pixelate" | "blackout";

/** Gaussian radius / pixelate block size, in scene (CSS) px. */
export const DEFAULT_BLUR_RADIUS = 12;
export const MIN_BLUR_RADIUS = 2;
export const MAX_BLUR_RADIUS = 60;

/** Even the softest "blur" must destroy legible text (redaction honesty). */
export const SECURE_BLUR_MIN = 10;

/** Highlighter tint opacity. */
export const HIGHLIGHTER_ALPHA = 0.36;
/** Spotlight dim colour (outside the hole). */
export const SPOTLIGHT_DIM = "rgba(8, 12, 18, 0.68)";
/** Default blackout ink. */
export const BLACKOUT_INK = "#111827";

export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** A rectangle snapped to integer device pixels. */
export interface DeviceRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

export const clampBlurRadius = (value: number): number => {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    return DEFAULT_BLUR_RADIUS;
  }
  return Math.round(Math.min(Math.max(value, MIN_BLUR_RADIUS), MAX_BLUR_RADIUS));
};

/**
 * Project a scene-space rectangle onto the flattened layer's pixel grid.
 * `box` is the export viewport (crop or whole image) in scene coords.
 */
export const toDevice = (
  region: Rect,
  box: Rect,
  multiplier: number,
  layer: { width: number; height: number },
): DeviceRect => {
  const scale = multiplier > 0 ? multiplier : 1;
  const x = Math.round((region.left - box.left) * scale);
  const y = Math.round((region.top - box.top) * scale);
  const right = Math.round((region.left + region.width - box.left) * scale);
  const bottom = Math.round((region.top + region.height - box.top) * scale);
  // Clamp into the layer; a region dragged past the edge must not throw.
  const left = Math.max(0, Math.min(x, layer.width));
  const top = Math.max(0, Math.min(y, layer.height));
  const clippedRight = Math.max(left, Math.min(right, layer.width));
  const clippedBottom = Math.max(top, Math.min(bottom, layer.height));
  return {
    x: left,
    y: top,
    width: clippedRight - left,
    height: clippedBottom - top,
  };
};

const scratchCanvas = (width: number, height: number) => {
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(width));
  canvas.height = Math.max(1, Math.round(height));
  return canvas;
};

/** Rounded-rectangle path that works on WebKitGTK builds without `roundRect`. */
const roundedRectPath = (
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  radius: number,
): void => {
  const r = Math.max(0, Math.min(radius, Math.min(width, height) / 2));
  context.moveTo(x + r, y);
  context.lineTo(x + width - r, y);
  context.arcTo(x + width, y, x + width, y + r, r);
  context.lineTo(x + width, y + height - r);
  context.arcTo(x + width, y + height, x + width - r, y + height, r);
  context.lineTo(x + r, y + height);
  context.arcTo(x, y + height, x, y + height - r, r);
  context.lineTo(x, y + r);
  context.arcTo(x, y, x + r, y, r);
  context.closePath();
};

/**
 * Alpha mask for a feathered rectangle: opaque in the middle, ramping to fully
 * transparent over `feather` px at each edge. Built with destination-out
 * gradients (no canvas filters, so it behaves identically everywhere).
 */
const featherMask = (width: number, height: number, feather: number): HTMLCanvasElement => {
  const mask = scratchCanvas(width, height);
  const context = mask.getContext("2d");
  if (!context) return mask;

  context.fillStyle = "#ffffff";
  context.fillRect(0, 0, width, height);

  const f = Math.max(1, Math.min(Math.round(feather), Math.floor(Math.min(width, height) / 2)));
  if (f < 1) return mask;

  context.globalCompositeOperation = "destination-out";

  const strip = (
    x: number,
    y: number,
    w: number,
    h: number,
    gradient: CanvasGradient,
  ): void => {
    context.fillStyle = gradient;
    context.fillRect(x, y, w, h);
  };

  const left = context.createLinearGradient(0, 0, f, 0);
  left.addColorStop(0, "rgba(0, 0, 0, 1)");
  left.addColorStop(1, "rgba(0, 0, 0, 0)");
  strip(0, 0, f, height, left);

  const right = context.createLinearGradient(width, 0, width - f, 0);
  right.addColorStop(0, "rgba(0, 0, 0, 1)");
  right.addColorStop(1, "rgba(0, 0, 0, 0)");
  strip(width - f, 0, f, height, right);

  const top = context.createLinearGradient(0, 0, 0, f);
  top.addColorStop(0, "rgba(0, 0, 0, 1)");
  top.addColorStop(1, "rgba(0, 0, 0, 0)");
  strip(0, 0, width, f, top);

  const bottom = context.createLinearGradient(0, height, 0, height - f);
  bottom.addColorStop(0, "rgba(0, 0, 0, 1)");
  bottom.addColorStop(1, "rgba(0, 0, 0, 0)");
  strip(0, height - f, width, f, bottom);

  context.globalCompositeOperation = "source-over";
  return mask;
};

/**
 * REAL gaussian blur of `rect`, with feathered edges.
 *
 * The sample window is padded by 1.5x the radius and clipped to the layer, so
 * blur near a screenshot edge pulls in real neighbouring pixels instead of
 * decaying into transparency, and the feathered mask hides the seam where the
 * blurred patch meets untouched pixels.
 */
export const gaussianBlur = (
  layer: HTMLCanvasElement,
  rect: DeviceRect,
  radius: number,
): void => {
  const width = Math.floor(rect.width);
  const height = Math.floor(rect.height);
  if (width < 2 || height < 2) return;
  const blurRadius = Math.max(1, Math.round(radius));

  const pad = Math.ceil(blurRadius * 1.5) + 2;
  const px = Math.max(0, rect.x - pad);
  const py = Math.max(0, rect.y - pad);
  const pw = Math.min(layer.width - px, width + pad * 2);
  const ph = Math.min(layer.height - py, height + pad * 2);
  if (pw < 2 || ph < 2) return;

  const source = scratchCanvas(pw, ph);
  const sourceContext = source.getContext("2d", { willReadFrequently: true });
  if (!sourceContext) return;
  sourceContext.drawImage(layer, px, py, pw, ph, 0, 0, pw, ph);

  const pixels = sourceContext.getImageData(0, 0, pw, ph);
  // StackBlur is the workhorse: O(n) triangle-convolution gaussian.
  imageDataRGBA(pixels, 0, 0, pw, ph, blurRadius);
  sourceContext.putImageData(pixels, 0, 0);

  const patch = scratchCanvas(width, height);
  const patchContext = patch.getContext("2d");
  if (!patchContext) return;
  patchContext.drawImage(source, -(rect.x - px), -(rect.y - py));

  const feather = Math.max(2, Math.min(Math.round(blurRadius * 0.9), Math.floor(Math.min(width, height) / 3)));
  patchContext.globalCompositeOperation = "destination-in";
  patchContext.drawImage(featherMask(width, height, feather), 0, 0);
  patchContext.globalCompositeOperation = "source-over";

  const context = layer.getContext("2d");
  if (!context) return;
  context.save();
  context.drawImage(patch, rect.x, rect.y);
  context.restore();
};

/** Block pixelation (downscale + nearest-neighbour upscale). */
export const pixelate = (
  layer: HTMLCanvasElement,
  rect: DeviceRect,
  block: number,
): void => {
  const width = Math.floor(rect.width);
  const height = Math.floor(rect.height);
  if (width < 2 || height < 2) return;
  const size = Math.max(2, Math.round(block));

  const small = scratchCanvas(Math.max(1, Math.round(width / size)), Math.max(1, Math.round(height / size)));
  const smallContext = small.getContext("2d");
  if (!smallContext) return;
  smallContext.imageSmoothingEnabled = true;
  smallContext.drawImage(layer, rect.x, rect.y, width, height, 0, 0, small.width, small.height);

  const context = layer.getContext("2d");
  if (!context) return;
  context.save();
  context.imageSmoothingEnabled = false;
  context.drawImage(small, 0, 0, small.width, small.height, rect.x, rect.y, width, height);
  context.restore();
};

/** Opaque redaction — nothing survives. */
export const blackout = (
  layer: HTMLCanvasElement,
  rect: DeviceRect,
  ink: string,
): void => {
  const context = layer.getContext("2d");
  if (!context) return;
  context.save();
  context.fillStyle = ink || BLACKOUT_INK;
  context.fillRect(rect.x, rect.y, Math.floor(rect.width), Math.floor(rect.height));
  context.restore();
};

/** Dispatch one blur region through its mode. `radius` is in device px. */
export const applyBlurRegion = (
  layer: HTMLCanvasElement,
  rect: DeviceRect,
  mode: BlurMode,
  radius: number,
  ink = BLACKOUT_INK,
): void => {
  if (rect.width < 1 || rect.height < 1) return;
  switch (mode) {
    case "pixelate":
      pixelate(layer, rect, Math.max(2, radius * 0.9));
      return;
    case "blackout":
      blackout(layer, rect, ink);
      return;
    case "gaussian":
    default:
      gaussianBlur(layer, rect, radius);
  }
};

/**
 * Spotlight: dim the whole layer except `holes`. Even-odd fill gives clean
 * holes without any temporary canvas juggling.
 */
export const spotlight = (
  layer: HTMLCanvasElement,
  holes: DeviceRect[],
  dim: string = SPOTLIGHT_DIM,
): void => {
  const usable = holes.filter((hole) => hole.width > 0 && hole.height > 0);
  if (!usable.length) return;
  const context = layer.getContext("2d");
  if (!context) return;

  context.save();
  context.beginPath();
  context.rect(0, 0, layer.width, layer.height);
  for (const hole of usable) {
    // Slight corner rounding so the lit area reads as a deliberate spotlight.
    const radius = Math.min(12, Math.min(hole.width, hole.height) / 6);
    const x = hole.x;
    const y = hole.y;
    // Sub-paths must be separate: moveTo starts a new contour.
    context.moveTo(x + radius, y);
    roundedRectPath(context, x, y, hole.width, hole.height, radius);
  }
  context.fillStyle = dim;
  context.fill("evenodd");
  context.restore();
};

/** Translucent highlighter tint, multiplied into the image pixels. */
export const highlighter = (
  layer: HTMLCanvasElement,
  rect: DeviceRect,
  ink: string,
  alpha = HIGHLIGHTER_ALPHA,
): void => {
  if (rect.width < 1 || rect.height < 1) return;
  const context = layer.getContext("2d");
  if (!context) return;
  context.save();
  context.globalCompositeOperation = "multiply";
  context.globalAlpha = Math.min(Math.max(alpha, 0), 1);
  context.fillStyle = ink || "#ffe066";
  const radius = Math.min(8, Math.min(rect.width, rect.height) / 4);
  if (radius > 0.5) {
    context.beginPath();
    roundedRectPath(context, rect.x, rect.y, rect.width, rect.height, radius);
    context.fill();
  } else {
    context.fillRect(rect.x, rect.y, rect.width, rect.height);
  }
  context.restore();
};