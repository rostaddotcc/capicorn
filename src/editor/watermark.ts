/**
 * Optional watermark burned into exports.
 *
 * Drawn on the export canvas only — the editor stays clean, so the user always
 * sees the real annotations, and toggling the mark on or off never destroys
 * anything. Text-only on purpose: a logo upload is a different feature (it would
 * need its own asset pipeline), while a name/handle covers the actual need.
 */
export interface WatermarkOptions {
  /** Text to draw; empty or whitespace disables the watermark. */
  text: string;
  /** 0..1 — how visible the mark is. */
  opacity: number;
  /** Font size in export pixels. */
  size: number;
  /** Which corner the mark sits in. */
  corner: WatermarkCorner;
  /** CSS colour for the text. */
  color: string;
}

export type WatermarkCorner =
  | "top-left"
  | "top-right"
  | "bottom-left"
  | "bottom-right";

export const DEFAULT_WATERMARK: WatermarkOptions = {
  text: "",
  opacity: 0.45,
  size: 22,
  corner: "bottom-right",
  color: "#ffffff",
};

/** Clamp untrusted settings (localStorage, older releases) into a safe shape. */
export const normalizeWatermark = (value: Partial<WatermarkOptions>): WatermarkOptions => ({
  text: typeof value.text === "string" ? value.text.slice(0, 120) : "",
  opacity: Number.isFinite(value.opacity as number)
    ? Math.min(1, Math.max(0.02, value.opacity as number))
    : DEFAULT_WATERMARK.opacity,
  size: Number.isFinite(value.size as number)
    ? Math.min(400, Math.max(8, value.size as number))
    : DEFAULT_WATERMARK.size,
  corner: (
    ["top-left", "top-right", "bottom-left", "bottom-right"] as const
  ).includes(value.corner as WatermarkCorner)
    ? (value.corner as WatermarkCorner)
    : DEFAULT_WATERMARK.corner,
  color:
    typeof value.color === "string" && /^#[0-9a-f]{6}$/i.test(value.color)
      ? value.color
      : DEFAULT_WATERMARK.color,
});

/**
 * Paint the watermark onto an export canvas (native resolution).
 *
 * A soft dark shadow keeps light text readable on light screenshots, which is
 * the common case for a captured UI.
 */
export const drawWatermark = (
  context: CanvasRenderingContext2D,
  width: number,
  height: number,
  options: WatermarkOptions,
): void => {
  const text = options.text.trim();
  if (!text || width <= 0 || height <= 0) return;

  const size = Math.max(8, options.size);
  const pad = Math.round(size * 0.9);

  context.save();
  context.globalAlpha = Math.min(1, Math.max(0.02, options.opacity));
  context.font = `600 ${size}px Inter, "Source Sans 3", system-ui, sans-serif`;
  context.textBaseline = "alphabetic";

  const metrics = context.measureText(text);
  const x = options.corner.endsWith("right")
    ? Math.max(pad, width - pad - metrics.width)
    : pad;
  const y = options.corner.startsWith("bottom")
    ? Math.max(size + pad, height - pad)
    : pad + size;

  context.shadowColor = "rgba(0, 0, 0, 0.5)";
  context.shadowBlur = Math.max(2, size * 0.18);
  context.shadowOffsetY = Math.max(1, size * 0.04);
  context.fillStyle = options.color;
  context.fillText(text, x, y);
  context.restore();
};
