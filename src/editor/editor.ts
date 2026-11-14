// Capicorn annotation editor.
//
// Fabric.js 6 canvas wrapper: loads a captured screenshot, fits it to the
// container, hosts the annotation tools (src/editor/tools.ts), keeps an
// undo/redo history of canvas states and flattens everything — image +
// annotations + crop + pixelated blur regions — into a PNG data URL.
//
// Public API is LOCKED (editor module ↔ app shell contract, spec §4.2).

import * as fabric from "fabric";
import {
  DEFAULT_BLUR_RADIUS,
  MAX_BLUR_RADIUS,
  MIN_BLUR_RADIUS,
  applyBlurRegion,
  clampBlurRadius,
  highlighter as applyHighlighter,
  spotlight as applySpotlight,
  toDevice,
  type BlurMode,
  type DeviceRect,
} from "./blur";
import {
  ACCENT,
  MAGNIFY_ZOOM,
  MAX_STROKE_WIDTH,
  MIN_STROKE_WIDTH,
  STROKE_WIDTH,
  blurRegionPreview,
  clampMagnifyZoom,
  type Box,
  type ImageInfo,
  type Point,
  type TextStyle,
  type Tool,
  type ToolHost,
  createTool,
  drawMagnify,
  fitBox,
  isBlurRegion,
  isHighlighterRegion,
  isMagnifyRegion,
  isSpotlightRegion,
} from "./tools";

export type { BlurMode } from "./blur";

export type ToolId =
  | "select"
  | "crop"
  | "arrow"
  | "text"
  | "rect"
  | "blur"
  | "spotlight"
  | "highlighter"
  | "callout"
  | "step"
  | "bucket"
  | "magnify"
  | "brush";

const TOOL_IDS: readonly ToolId[] = [
  "select",
  "crop",
  "arrow",
  "text",
  "rect",
  "blur",
  "spotlight",
  "highlighter",
  "callout",
  "step",
  "bucket",
  "magnify",
  "brush",
];

/**
 * The editor's "ink" settings — exactly what a named style preset stores.
 *
 * The editor only reads and writes the LIVE values; the app shell owns preset
 * persistence (named styles, favourites), so nothing here touches storage.
 * Plain JSON, so a preset round-trips through localStorage unharmed.
 */
export interface EditorStyle {
  /** Ink colour for new annotations (and highlighter fill). */
  color: string;
  /** Stroke width for new shapes/arrows, 1..12. */
  strokeWidth: number;
  /** Caption font family. */
  fontFamily: string;
  /** Caption font size, 8..200. */
  fontSize: number;
  bold: boolean;
  italic: boolean;
  /** Redaction mode for NEW blur regions. */
  blurMode: BlurMode;
  /** Gaussian radius / pixelate block size, 2..60 scene px. */
  blurRadius: number;
}

const MAX_HISTORY = 80;
/** Breathing room around the screenshot inside the canvas. */
const PADDING = 16;
/** Smallest useful crop / annotation. */
const MIN_SIZE = 8;
/** Never export more than this on the long side. */
const MAX_EXPORT_EDGE = 8192;
/** Caption defaults and size limits. */
const DEFAULT_FONT_FAMILY = "Inter";
const DEFAULT_FONT_SIZE = 32;
const MIN_FONT_SIZE = 8;
const MAX_FONT_SIZE = 200;
/** Freehand brush width limits and default (scene px). */
const DEFAULT_BRUSH_WIDTH = 4;
const MIN_BRUSH_WIDTH = 1;
const MAX_BRUSH_WIDTH = 60;
/** Step-badge diameter limits and default (scene px). */
const DEFAULT_STEP_SIZE = 30;
const MIN_STEP_SIZE = 16;
const MAX_STEP_SIZE = 64;
/** Callout caption size limits and default (scene px). */
const DEFAULT_CALLOUT_FONT_SIZE = 24;
const MIN_CALLOUT_FONT_SIZE = 12;
const MAX_CALLOUT_FONT_SIZE = 120;
/** Spotlight / highlighter outline thickness limits and default (scene px). */
const DEFAULT_REGION_THICKNESS = 2;
const MIN_REGION_THICKNESS = 1;
const MAX_REGION_THICKNESS = 60;

/** Range a tool's size slider spans; `step` is the increment the UI steps by. */
export interface ToolSizeRange {
  min: number;
  max: number;
  step: number;
}

/**
 * The one size each tool exposes. Every tool maps its size onto the setting that
 * actually changes what you draw, so a single slider drives all of them:
 * brush→stroke width, arrow/rect→stroke width, text→font size,
 * blur→radius, step→badge diameter, callout→caption size,
 * spotlight/highlighter→outline thickness, magnify→zoom.
 * Tools with no meaningful size (select, crop, bucket) report null.
 */
const TOOL_SIZE_RANGES: Record<ToolId, ToolSizeRange | null> = {
  select: null,
  crop: null,
  arrow: { min: MIN_STROKE_WIDTH, max: MAX_STROKE_WIDTH, step: 1 },
  text: { min: MIN_FONT_SIZE, max: MAX_FONT_SIZE, step: 1 },
  rect: { min: MIN_STROKE_WIDTH, max: MAX_STROKE_WIDTH, step: 1 },
  blur: { min: MIN_BLUR_RADIUS, max: MAX_BLUR_RADIUS, step: 1 },
  spotlight: { min: MIN_REGION_THICKNESS, max: MAX_REGION_THICKNESS, step: 1 },
  highlighter: { min: MIN_REGION_THICKNESS, max: MAX_REGION_THICKNESS, step: 1 },
  callout: { min: MIN_CALLOUT_FONT_SIZE, max: MAX_CALLOUT_FONT_SIZE, step: 1 },
  step: { min: MIN_STEP_SIZE, max: MAX_STEP_SIZE, step: 1 },
  bucket: null,
  magnify: { min: 1.2, max: 6, step: 0.1 },
  brush: { min: MIN_BRUSH_WIDTH, max: MAX_BRUSH_WIDTH, step: 1 },
};

/** Snap a value into a range on its own step grid. */
const clampToRange = (value: number, range: ToolSizeRange): number => {
  if (typeof value !== "number" || !Number.isFinite(value)) return range.min;
  const step = range.step > 0 ? range.step : 1;
  const snapped = Math.round((value - range.min) / step) * step + range.min;
  const clamped = Math.min(Math.max(snapped, range.min), range.max);
  return Number(clamped.toFixed(step < 1 ? 1 : 0));
};

interface HistoryEntry {
  json: Record<string, unknown>;
  crop: Box | null;
  /** Natural pixel size of the screenshot at the time of the entry. */
  natural: { width: number; height: number };
}

const isToolId = (value: string): value is ToolId =>
  (TOOL_IDS as readonly string[]).includes(value);

/** Objects that are swapped for pixels on export instead of being drawn. */
const isEffectRegion = (object: fabric.FabricObject): boolean =>
  isBlurRegion(object) ||
  isSpotlightRegion(object) ||
  isHighlighterRegion(object) ||
  isMagnifyRegion(object);

export class Editor {
  private readonly container: HTMLElement;
  private readonly canvas: fabric.Canvas;
  private readonly host: ToolHost;
  private readonly tools = new Map<ToolId, Tool>();
  /** Editor-only decoration (crop shading) — never exported, never in history. */
  private readonly chrome = new Set<fabric.FabricObject>();

  private image: fabric.FabricImage | null = null;
  private sourceDataUrl = "";
  private natural = { width: 1, height: 1 };
  /**
   * The capture as decoded pixels. resizeImage() re-samples THIS, so repeated
   * resizes copy from the original rather than compounding re-encodes, and
   * undo can rebuild any earlier size.
   */
  private base: CanvasImageSource | null = null;

  private tool: ToolId = "select";
  private cropBox: Box | null = null;

  /** Ink colour applied to NEW annotations (existing ones keep their colour). */
  private inkColor = ACCENT;
  /** Stroke width applied to NEW annotations. */
  private inkWidth = STROKE_WIDTH;
  /** Redaction mode applied to NEW blur/redaction regions. */
  private blurMode: BlurMode = "gaussian";
  /** Gaussian radius (or pixelate block size) in scene px, for NEW regions. */
  private blurRadius = DEFAULT_BLUR_RADIUS;
  /** Caption typography applied to NEW text objects. */
  private fontFamily = DEFAULT_FONT_FAMILY;
  private fontSize = DEFAULT_FONT_SIZE;
  private textBold = false;
  private textItalic = false;
  /** Freehand stroke width for NEW brush strokes. */
  private brushWidth = DEFAULT_BRUSH_WIDTH;
  /** Diameter of NEW numbered step badges. */
  private stepSize = DEFAULT_STEP_SIZE;
  /** Caption font size inside NEW callouts (independent of caption size). */
  private calloutFontSize = DEFAULT_CALLOUT_FONT_SIZE;
  /** Outline thickness for NEW spotlight / highlighter regions. */
  private regionThickness = DEFAULT_REGION_THICKNESS;
  /** Magnification for NEW lenses. */
  private magnifyZoom = MAGNIFY_ZOOM;

  private history: HistoryEntry[] = [];
  private cursor = -1;
  /** True while a history state is being applied — blocks recursive commits. */
  private restoring = false;
  private pending: Promise<void> = Promise.resolve();

  private readonly resizeObserver: ResizeObserver | null;
  private destroyed = false;

  constructor(container: HTMLElement) {
    this.container = container;

    const element = document.createElement("canvas");
    container.replaceChildren(element);

    this.canvas = new fabric.Canvas(element, {
      preserveObjectStacking: true,
      selection: true,
      uniformScaling: false,
      enableRetinaScaling: true,
      stopContextMenu: true,
      fireRightClick: false,
    });

    this.host = {
      canvas: this.canvas,
      bounds: () => this.drawBounds(),
      imageInfo: () => this.imageInfo(),
      textStyle: () => this.textStyle(),
      addObject: (object) => this.addObject(object),
      removeObject: (object) => this.removeObject(object),
      commit: () => this.commit(),
      setCrop: (box) => this.setCrop(box),
      nextStepNumber: () => this.nextStepNumber(),
      color: () => this.inkColor,
      strokeWidth: () => this.inkWidth,
      blurMode: () => this.blurMode,
      blurRadius: () => this.blurRadius,
      brushWidth: () => this.brushWidth,
      stepSize: () => this.stepSize,
      calloutFontSize: () => this.calloutFontSize,
      regionThickness: () => this.regionThickness,
      magnifyZoom: () => this.magnifyZoom,
    };

    this.canvas.on("mouse:down", this.onMouseDown);
    this.canvas.on("mouse:move", this.onMouseMove);
    this.canvas.on("mouse:up", this.onMouseUp);
    this.canvas.on("object:modified", this.onObjectModified);

    window.addEventListener("keydown", this.onKeyDown, true);

    this.resizeObserver =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => this.layout());
    this.resizeObserver?.observe(container);

    this.layout(true);
    this.currentTool().activate();
    this.applyInteractivity();
  }

  /* ------------------------------------------------------------- public API */

  /** Load a captured image (PNG data URL) into the editor, fit to view. */
  async load(dataUrl: string): Promise<void> {
    if (this.destroyed) return;
    this.sourceDataUrl = dataUrl;

    const image = await fabric.FabricImage.fromURL(dataUrl);
    if (this.destroyed) return;

    this.canvas.discardActiveObject();
    const stale = this.canvas.getObjects();
    if (stale.length) this.canvas.remove(...stale);
    this.chrome.clear();
    this.cropBox = null;
    this.history = [];
    this.cursor = -1;

    const width = image.width ?? this.natural.width;
    const height = image.height ?? this.natural.height;
    this.natural = { width: Math.max(1, width), height: Math.max(1, height) };

    // Keep the decoded capture: resizeImage() always re-samples from here.
    const element = image.getElement();
    this.base = (element ?? null) as CanvasImageSource | null;

    this.configureImage(image);

    this.image = image;
    this.canvas.add(image);
    this.layout(true);
    this.resetHistory();
    this.applyInteractivity();
    this.canvas.requestRenderAll();
    this.notify();
  }

  /** Flatten image + annotations (and crop, and blur) to a PNG data URL. */
  flatten(): string {
    const image = this.image;
    if (!image) return this.sourceDataUrl;

    const box = this.outputBox();
    const multiplier = this.outputMultiplier(box);

    // Settle an in-flight caption first (so it lands in history), then silence
    // history while the render state is mutated.
    const active = this.canvas.getActiveObject();
    if (active instanceof fabric.IText && active.isEditing) {
      const wasRestoring = this.restoring;
      this.restoring = false;
      active.exitEditing();
      this.restoring = wasRestoring;
    }

    const wasRestoring = this.restoring;
    this.restoring = true;

    const effects = this.canvas.getObjects().filter(isEffectRegion);
    const hidden: fabric.FabricObject[] = [...effects, ...this.chrome];
    const visibility = hidden.map((object) => object.visible);
    for (const object of hidden) object.visible = false;

    let imageLayer: HTMLCanvasElement;
    let inkLayer: HTMLCanvasElement;
    try {
      imageLayer = this.canvas.toCanvasElement(multiplier, {
        ...box,
        filter: (object) => object === image,
      });
      inkLayer = this.canvas.toCanvasElement(multiplier, {
        ...box,
        filter: (object) => !object.excludeFromExport,
      });
    } finally {
      for (let index = 0; index < hidden.length; index += 1) {
        const object = hidden[index];
        object.visible = visibility[index];
      }
      this.restoring = wasRestoring;
    }

    this.applyEffects(imageLayer, effects, box, multiplier);

    const output = document.createElement("canvas");
    output.width = Math.max(1, imageLayer.width);
    output.height = Math.max(1, imageLayer.height);
    const context = output.getContext("2d");
    if (!context) return this.sourceDataUrl;
    context.fillStyle = "#ffffff";
    context.fillRect(0, 0, output.width, output.height);
    context.drawImage(imageLayer, 0, 0);
    context.drawImage(inkLayer, 0, 0);
    return output.toDataURL("image/png");
  }

  setTool(tool: ToolId): void {
    if (!isToolId(tool)) return;
    const previous = this.tools.get(this.tool);
    if (previous && this.tool !== tool) previous.deactivate();
    this.tool = tool;
    const next = this.currentTool();
    next.activate();
    this.applyInteractivity();
    this.canvas.defaultCursor = next.cursor;
    this.canvas.requestRenderAll();
    this.notify();
  }

  undo(): void {
    if (this.cursor <= 0) return;
    this.cursor -= 1;
    this.restore();
  }

  redo(): void {
    if (this.cursor >= this.history.length - 1) return;
    this.cursor += 1;
    this.restore();
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.resizeObserver?.disconnect();
    window.removeEventListener("keydown", this.onKeyDown, true);
    for (const tool of this.tools.values()) tool.deactivate();
    this.tools.clear();
    this.chrome.clear();
    void this.canvas.dispose();
    this.container.replaceChildren();
  }

  /* ------------------------------------------- additive UI conveniences */

  getTool(): ToolId {
    return this.tool;
  }

  canUndo(): boolean {
    return this.cursor > 0;
  }

  canRedo(): boolean {
    return this.cursor < this.history.length - 1;
  }

  /** Ink colour for new annotations. Does not recolour existing objects. */
  setColor(color: string): void {
    if (typeof color !== "string") return;
    const next = color.trim();
    if (!next) return;
    this.inkColor = next;
    this.notify();
  }

  getColor(): string {
    return this.inkColor;
  }

  /** Stroke width for new annotations, clamped to 1..12 and rounded. */
  setStrokeWidth(width: number): void {
    if (typeof width !== "number" || !Number.isFinite(width)) return;
    const clamped = Math.min(Math.max(width, MIN_STROKE_WIDTH), MAX_STROKE_WIDTH);
    this.inkWidth = Math.round(clamped);
    this.notify();
  }

  getStrokeWidth(): number {
    return this.inkWidth;
  }

  /**
   * Redaction mode for NEW blur regions. If blur regions are selected, they are
   * retuned in place as well (so an existing region can be flipped between
   * gaussian / pixelate / blackout without redrawing it).
   */
  setBlurMode(mode: BlurMode): void {
    if (mode !== "gaussian" && mode !== "pixelate" && mode !== "blackout") return;
    const changed = this.blurMode !== mode;
    this.blurMode = mode;
    const retuned = this.retuneBlurSelection();
    if (changed && !retuned) this.notify();
  }

  getBlurMode(): BlurMode {
    return this.blurMode;
  }

  /** Gaussian radius / pixelate block size, in scene px, clamped to 2..60. */
  setBlurRadius(radius: number): void {
    if (typeof radius !== "number" || !Number.isFinite(radius)) return;
    const next = clampBlurRadius(radius);
    const changed = this.blurRadius !== next;
    this.blurRadius = next;
    const retuned = this.retuneBlurSelection();
    if (changed && !retuned) this.notify();
  }

  getBlurRadius(): number {
    return this.blurRadius;
  }

  /** Apply the current mode/radius to selected blur regions. True if any. */
  private retuneBlurSelection(): boolean {
    if (this.restoring) return false;
    const targets = this.canvas.getActiveObjects().filter(isBlurRegion);
    if (!targets.length) return false;
    for (const region of targets) {
      region.mode = this.blurMode;
      region.radius = this.blurRadius;
      const preview = blurRegionPreview(region.mode);
      region.set({ fill: preview.fill, stroke: preview.stroke });
      region.dirty = true;
    }
    this.commit();
    this.canvas.requestRenderAll();
    return true;
  }

  /** True once the canvas holds any state beyond the freshly loaded capture. */
  isDirty(): boolean {
    return this.cursor > 0;
  }

  /** Snapshot of the current ink settings (the shell saves this as a preset). */
  getStyle(): EditorStyle {
    return {
      color: this.inkColor,
      strokeWidth: this.inkWidth,
      fontFamily: this.fontFamily,
      fontSize: this.fontSize,
      bold: this.textBold,
      italic: this.textItalic,
      blurMode: this.blurMode,
      blurRadius: this.blurRadius,
    };
  }

  /**
   * Restore a preset. Only the fields present in `style` are applied, each
   * through its normal validating setter (so out-of-range or junk values from
   * stored presets are clamped/ignored), and one change event is emitted when
   * anything actually moved. Existing annotations are never recoloured.
   */
  applyStyle(style: Partial<EditorStyle> | null | undefined): void {
    if (!style || typeof style !== "object") return;

    const isInk = (value: unknown): value is string =>
      typeof value === "string" && value.trim().length > 0;
    const isNumber = (value: unknown): value is number =>
      typeof value === "number" && Number.isFinite(value);

    const before = JSON.stringify(this.getStyle());

    if (isInk(style.color)) this.setColor(style.color);
    if (isNumber(style.strokeWidth)) this.setStrokeWidth(style.strokeWidth);
    if (isInk(style.fontFamily)) this.setFontFamily(style.fontFamily);
    if (isNumber(style.fontSize)) this.setFontSize(style.fontSize);
    if (typeof style.bold === "boolean") this.setTextBold(style.bold);
    if (typeof style.italic === "boolean") this.setTextItalic(style.italic);
    if (style.blurMode) this.setBlurMode(style.blurMode);
    if (isNumber(style.blurRadius)) this.setBlurRadius(style.blurRadius);

    // The setters above already notify; this covers the no-op-but-changed case
    // (e.g. a preset that only matched already-active values) exactly once.
    if (JSON.stringify(this.getStyle()) !== before) this.notify();
  }

  /** Font family for new captions. Existing captions keep their own. */
  setFontFamily(fontFamily: string): void {
    if (typeof fontFamily !== "string") return;
    const next = fontFamily.trim();
    if (!next) return;
    this.fontFamily = next;
    this.notify();
  }

  getFontFamily(): string {
    return this.fontFamily;
  }

  /** Caption font size for new text, clamped to 8..200 and rounded. */
  setFontSize(size: number): void {
    if (typeof size !== "number" || !Number.isFinite(size)) return;
    const clamped = Math.min(Math.max(size, MIN_FONT_SIZE), MAX_FONT_SIZE);
    this.fontSize = Math.round(clamped);
    this.notify();
  }

  getFontSize(): number {
    return this.fontSize;
  }

  setTextBold(bold: boolean): void {
    if (this.textBold === !!bold) return;
    this.textBold = !!bold;
    this.notify();
  }

  isTextBold(): boolean {
    return this.textBold;
  }

  setTextItalic(italic: boolean): void {
    if (this.textItalic === !!italic) return;
    this.textItalic = !!italic;
    this.notify();
  }

  isTextItalic(): boolean {
    return this.textItalic;
  }

  /* ------------------------------------------------------- unified tool size */

  /**
   * Size range for the ACTIVE tool, or null when the tool has no size
   * (select, crop, bucket). `step` is the increment the UI should use.
   */
  getToolSizeRange(): ToolSizeRange | null {
    const range = TOOL_SIZE_RANGES[this.tool];
    return range ? { ...range } : null;
  }

  /** Active tool's size — 0 for tools without one (check getToolSizeRange). */
  getToolSize(): number {
    switch (this.tool) {
      case "brush":
        return this.brushWidth;
      case "arrow":
      case "rect":
        return this.inkWidth;
      case "text":
        return this.fontSize;
      case "blur":
        return this.blurRadius;
      case "step":
        return this.stepSize;
      case "callout":
        return this.calloutFontSize;
      case "spotlight":
      case "highlighter":
        return this.regionThickness;
      case "magnify":
        return this.magnifyZoom;
      default:
        return 0;
    }
  }

  /**
   * Set the active tool's size, snapped to the tool's range. Routed through the
   * specific setters where one exists, so getToolSize() and getStrokeWidth() /
   * getFontSize() / getBlurRadius() can never disagree.
   */
  setToolSize(size: number): void {
    if (typeof size !== "number" || !Number.isFinite(size)) return;
    const range = TOOL_SIZE_RANGES[this.tool];
    if (!range) return;
    const value = clampToRange(size, range);

    switch (this.tool) {
      case "brush":
        this.brushWidth = Math.round(value);
        break;
      case "step":
        this.stepSize = Math.round(value);
        break;
      case "callout":
        this.calloutFontSize = Math.round(value);
        break;
      case "arrow":
      case "rect":
        this.setStrokeWidth(value);
        return;
      case "text":
        this.setFontSize(value);
        return;
      case "blur":
        this.setBlurRadius(value);
        return;
      case "spotlight":
      case "highlighter": {
        const changed = this.regionThickness !== value;
        this.regionThickness = value;
        if (this.retuneRegionSelection()) return;
        if (changed) this.notify();
        return;
      }
      case "magnify": {
        const zoom = clampMagnifyZoom(value);
        const changed = this.magnifyZoom !== zoom;
        this.magnifyZoom = zoom;
        if (this.retuneLensSelection()) return;
        if (changed) this.notify();
        return;
      }
      default:
        return;
    }
    this.notify();
  }

  /** Apply the active thickness to selected spotlight/highlighter regions. */
  private retuneRegionSelection(): boolean {
    if (this.restoring) return false;
    const targets = this.canvas
      .getActiveObjects()
      .filter((object) => isSpotlightRegion(object) || isHighlighterRegion(object));
    if (!targets.length) return false;
    for (const region of targets) {
      if (!isSpotlightRegion(region) && !isHighlighterRegion(region)) continue;
      region.thickness = this.regionThickness;
      region.set({
        // Spotlight paints its edge itself; the highlighter's edge is its stroke.
        strokeWidth: isHighlighterRegion(region) ? this.regionThickness : 0,
        strokeUniform: true,
      });
      region.dirty = true;
    }
    this.commit();
    this.canvas.requestRenderAll();
    return true;
  }

  /** Apply the active zoom to selected lenses. */
  private retuneLensSelection(): boolean {
    if (this.restoring) return false;
    const targets = this.canvas.getActiveObjects().filter(isMagnifyRegion);
    if (!targets.length) return false;
    for (const lens of targets) lens.zoom = this.magnifyZoom;
    this.commit();
    this.canvas.requestRenderAll();
    return true;
  }

  /* ----------------------------------------------------------- image resize */

  /** Natural (pixel) size of the loaded screenshot; 0x0 before load(). */
  getImageSize(): { width: number; height: number } {
    if (!this.image) return { width: 0, height: 0 };
    return {
      width: Math.max(1, Math.round(this.natural.width)),
      height: Math.max(1, Math.round(this.natural.height)),
    };
  }

  /**
   * Re-sample the screenshot to `width` x `height` pixels (each clamped to >= 1).
   *
   * The new pixels are drawn from the ORIGINAL capture, so repeated resizes do
   * not compound re-encodes. Annotations and the crop follow the image's new
   * on-screen box (layout() rebases them about the image origin), the natural
   * size is updated, and the change is committed, so undo restores both the
   * pixels and the annotation geometry.
   */
  resizeImage(width: number, height: number): void {
    if (this.destroyed || this.restoring) return;
    if (!this.image || !this.base) return;
    if (typeof width !== "number" || typeof height !== "number") return;
    if (!Number.isFinite(width) || !Number.isFinite(height)) return;

    const nextWidth = Math.max(1, Math.round(width));
    const nextHeight = Math.max(1, Math.round(height));
    if (
      nextWidth === Math.round(this.natural.width) &&
      nextHeight === Math.round(this.natural.height)
    ) {
      return;
    }

    const rebuilt = this.buildImage(nextWidth, nextHeight);
    if (!rebuilt) return;

    // Park the new layer where the old one sits; layout() then rescales the ink
    // from the old display box to the new one.
    const previous = this.imageBox();
    rebuilt.image.set({
      left: previous.left,
      top: previous.top,
      scaleX: previous.width / nextWidth,
      scaleY: previous.height / nextHeight,
    });
    rebuilt.image.setCoords();

    const stale = this.image;
    this.canvas.remove(stale);
    this.canvas.insertAt(0, rebuilt.image);
    this.image = rebuilt.image;
    this.natural = { width: nextWidth, height: nextHeight };
    this.sourceDataUrl = rebuilt.dataUrl;

    this.layout(true);
    this.applyInteractivity();
    this.commit();
    this.canvas.requestRenderAll();
  }

  /** Screenshot flags: never selectable, never serialized into history. */
  private configureImage(image: fabric.FabricImage): fabric.FabricImage {
    image.set({
      left: 0,
      top: 0,
      originX: "left",
      originY: "top",
      selectable: false,
      evented: false,
      hasControls: false,
      hasBorders: false,
      hoverCursor: "default",
      lockMovementX: true,
      lockMovementY: true,
      lockRotation: true,
      // The screenshot is placed through its own transform and is never part of
      // the serialized history (a capture can be many megabytes).
      excludeFromExport: true,
    });
    return image;
  }

  /** Re-sample the base capture at `width` x `height` as a fresh image layer. */
  private buildImage(
    width: number,
    height: number,
  ): { image: fabric.FabricImage; dataUrl: string } | null {
    const base = this.base;
    if (!base) return null;

    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(width));
    canvas.height = Math.max(1, Math.round(height));
    const context = canvas.getContext("2d");
    if (!context) return null;
    context.imageSmoothingEnabled = true;
    context.imageSmoothingQuality = "high";
    try {
      context.drawImage(base, 0, 0, canvas.width, canvas.height);
    } catch {
      return null;
    }

    const image = this.configureImage(new fabric.FabricImage(canvas));
    return { image, dataUrl: canvas.toDataURL("image/png") };
  }

  /* --------------------------------------------------------------- tool glue */

  private currentTool(): Tool {
    let tool = this.tools.get(this.tool);
    if (!tool) {
      tool = createTool(this.tool, this.host);
      this.tools.set(this.tool, tool);
    }
    return tool;
  }

  private pointOf(event: fabric.TPointerEventInfo): Point {
    const point = this.canvas.getScenePoint(event.e);
    return { x: point.x, y: point.y };
  }

  private readonly onMouseDown = (event: fabric.TPointerEventInfo): void => {
    if (this.destroyed || this.restoring) return;
    if (this.tool === "select") return;
    const active = this.canvas.getActiveObject();
    if (active instanceof fabric.IText && active.isEditing) {
      active.exitEditing();
      return;
    }
    this.currentTool().onPointerDown(this.pointOf(event), event.e);
  };

  private readonly onMouseMove = (event: fabric.TPointerEventInfo): void => {
    if (this.destroyed || this.restoring) return;
    if (this.tool === "select") return;
    this.currentTool().onPointerMove(this.pointOf(event));
  };

  private readonly onMouseUp = (event: fabric.TPointerEventInfo): void => {
    if (this.destroyed || this.restoring) return;
    if (this.tool === "select") return;
    this.currentTool().onPointerUp(this.pointOf(event));
  };

  private readonly onObjectModified = (): void => {
    this.commit();
  };

  private readonly onKeyDown = (event: KeyboardEvent): void => {
    if (this.destroyed) return;
    const target = event.target as HTMLElement | null;
    if (
      target &&
      (target.tagName === "INPUT" ||
        target.tagName === "TEXTAREA" ||
        target.isContentEditable)
    ) {
      return;
    }

    const active = this.canvas.getActiveObject();
    if (active instanceof fabric.IText && active.isEditing) return;

    const modifier = event.ctrlKey || event.metaKey;
    const key = event.key.toLowerCase();

    if (modifier && key === "z") {
      event.preventDefault();
      if (event.shiftKey) this.redo();
      else this.undo();
      return;
    }
    if (modifier && key === "y") {
      event.preventDefault();
      this.redo();
      return;
    }
    if (event.key === "Escape") {
      if (this.currentTool().onEscape()) {
        event.preventDefault();
        return;
      }
      if (this.tool === "crop" && this.cropBox) {
        event.preventDefault();
        this.setCrop(null);
        return;
      }
      if (this.canvas.getActiveObject()) {
        event.preventDefault();
        this.canvas.discardActiveObject();
        this.canvas.requestRenderAll();
      }
      return;
    }
    if (event.key === "Delete" || event.key === "Backspace") {
      const targets = this.canvas
        .getActiveObjects()
        .filter((object) => !object.excludeFromExport);
      if (!targets.length) return;
      event.preventDefault();
      this.canvas.discardActiveObject();
      for (const object of targets) this.canvas.remove(object);
      this.commit();
      this.canvas.requestRenderAll();
    }
  };

  /* ---------------------------------------------------------- object plumbing */

  private addObject(object: fabric.FabricObject): void {
    this.canvas.add(object);
    this.applyInteractivity(object);
  }

  private removeObject(object: fabric.FabricObject): void {
    this.chrome.delete(object);
    this.canvas.remove(object);
  }

  private bounds(): Box {
    return {
      left: 0,
      top: 0,
      width: this.canvas.getWidth(),
      height: this.canvas.getHeight(),
    };
  }

  /** Pointer clamp: draw inside the screenshot, not into the letterbox. */
  private drawBounds(): Box {
    if (!this.image) return this.bounds();
    return this.imageBox();
  }

  /** The screenshot's source data + scene transform (bucket sampling). */
  private imageInfo(): ImageInfo | null {
    const image = this.image;
    if (!image || !this.sourceDataUrl) return null;
    return {
      dataUrl: this.sourceDataUrl,
      natural: { width: this.natural.width, height: this.natural.height },
      left: image.left,
      top: image.top,
      scaleX: image.scaleX,
      scaleY: image.scaleY,
    };
  }

  /** Typography the text tool should use for a new caption. */
  private textStyle(): TextStyle {
    return {
      fontFamily: this.fontFamily,
      fontSize: this.fontSize,
      fontWeight: this.textBold ? "bold" : "normal",
      fontStyle: this.textItalic ? "italic" : "normal",
    };
  }

  private nextStepNumber(): number {
    let highest = 0;
    for (const object of this.canvas.getObjects()) {
      if (!(object instanceof fabric.Group)) continue;
      for (const child of object.getObjects()) {
        if (!(child instanceof fabric.Text)) continue;
        const value = Number.parseInt((child.text ?? "").trim(), 10);
        if (!Number.isNaN(value) && value > highest) highest = value;
      }
    }
    return highest + 1;
  }

  /** Enable fabric's own select/move only for annotations, only with the select tool. */
  private applyInteractivity(only?: fabric.FabricObject): void {
    const selecting = this.tool === "select";
    this.canvas.selection = selecting;
    this.canvas.skipTargetFind = !selecting;

    const objects = only ? [only] : this.canvas.getObjects();
    for (const object of objects) {
      const interactive = selecting && !object.excludeFromExport;
      object.selectable = interactive;
      object.evented = interactive;
      object.hoverCursor = interactive ? "move" : "default";
    }

    if (!selecting) this.canvas.discardActiveObject();
    this.canvas.requestRenderAll();
  }

  /* ------------------------------------------------------------------- crop */

  private setCrop(box: Box | null): void {
    if (this.restoring) return;
    if (!box) {
      if (!this.cropBox) return;
      this.cropBox = null;
      this.renderCropOverlay();
      this.commit();
      this.canvas.requestRenderAll();
      return;
    }
    const clamped = this.clampToImage(box);
    if (clamped.width < MIN_SIZE || clamped.height < MIN_SIZE) return;
    this.cropBox = clamped;
    this.renderCropOverlay();
    this.commit();
    this.canvas.requestRenderAll();
  }

  private clampToImage(box: Box): Box {
    const limit = this.imageBox();
    const left = Math.max(box.left, limit.left);
    const top = Math.max(box.top, limit.top);
    const right = Math.min(box.left + box.width, limit.left + limit.width);
    const bottom = Math.min(box.top + box.height, limit.top + limit.height);
    return {
      left,
      top,
      width: Math.max(1, right - left),
      height: Math.max(1, bottom - top),
    };
  }

  private renderCropOverlay(): void {
    for (const object of this.chrome) this.canvas.remove(object);
    this.chrome.clear();

    const crop = this.cropBox;
    if (!crop) return;

    const width = this.canvas.getWidth();
    const height = this.canvas.getHeight();
    const bands: Box[] = [
      { left: 0, top: 0, width, height: crop.top },
      {
        left: 0,
        top: crop.top + crop.height,
        width,
        height: height - (crop.top + crop.height),
      },
      { left: 0, top: crop.top, width: crop.left, height: crop.height },
      {
        left: crop.left + crop.width,
        top: crop.top,
        width: width - (crop.left + crop.width),
        height: crop.height,
      },
    ];

    for (const band of bands) {
      if (band.width <= 0 || band.height <= 0) continue;
      const shade = new fabric.Rect({
        left: band.left,
        top: band.top,
        width: band.width,
        height: band.height,
        originX: "left",
        originY: "top",
        fill: "rgba(8, 10, 14, 0.55)",
        strokeWidth: 0,
        selectable: false,
        evented: false,
        excludeFromExport: true,
        objectCaching: false,
      });
      this.chrome.add(shade);
      this.canvas.add(shade);
    }

    const frame = new fabric.Rect({
      left: crop.left,
      top: crop.top,
      width: crop.width,
      height: crop.height,
      originX: "left",
      originY: "top",
      fill: "transparent",
      stroke: ACCENT,
      strokeWidth: 2,
      strokeDashArray: [10, 6],
      selectable: false,
      evented: false,
      excludeFromExport: true,
      objectCaching: false,
    });
    this.chrome.add(frame);
    this.canvas.add(frame);
  }

  /* ----------------------------------------------------------------- layout */

  private imageBox(): Box {
    const image = this.image;
    if (!image) return this.bounds();
    return {
      left: image.left,
      top: image.top,
      width: (image.width ?? this.natural.width) * image.scaleX,
      height: (image.height ?? this.natural.height) * image.scaleY,
    };
  }

  private imageBoxFor(width: number, height: number): Box {
    return fitBox(this.natural, { left: 0, top: 0, width, height }, PADDING);
  }

  private layout(force = false): void {
    if (this.destroyed) return;
    const measuredWidth = Math.round(this.container.clientWidth);
    const measuredHeight = Math.round(this.container.clientHeight);
    const width = measuredWidth > 0 ? measuredWidth : Math.max(1, this.canvas.getWidth());
    const height = measuredHeight > 0 ? measuredHeight : Math.max(1, this.canvas.getHeight());

    if (!force && width === this.canvas.getWidth() && height === this.canvas.getHeight()) {
      return;
    }

    const image = this.image;
    if (!image) {
      this.canvas.setDimensions({ width, height });
      this.canvas.calcOffset();
      this.canvas.requestRenderAll();
      return;
    }

    const previous = this.imageBox();
    this.canvas.setDimensions({ width, height });
    this.canvas.calcOffset();

    const next = this.imageBoxFor(width, height);
    const scale = previous.width > 0 ? next.width / previous.width : 1;

    image.set({
      left: next.left,
      top: next.top,
      scaleX: next.width / this.natural.width,
      scaleY: next.height / this.natural.height,
    });
    image.setCoords();

    if (Math.abs(scale - 1) > 1e-6) {
      for (const object of this.canvas.getObjects()) {
        if (object === image) continue;
        const centre = object.getCenterPoint();
        const x = next.left + (centre.x - previous.left) * scale;
        const y = next.top + (centre.y - previous.top) * scale;
        object.scaleX *= scale;
        object.scaleY *= scale;
        object.setPositionByOrigin(new fabric.Point(x, y), "center", "center");
        object.setCoords();
        // Blur strength is authored in scene px, so it must follow the zoom too.
        if (isBlurRegion(object)) {
          object.radius = clampBlurRadius(object.radius * scale);
        }
      }
      const crop = this.cropBox;
      if (crop) {
        this.cropBox = {
          left: next.left + (crop.left - previous.left) * scale,
          top: next.top + (crop.top - previous.top) * scale,
          width: crop.width * scale,
          height: crop.height * scale,
        };
        this.renderCropOverlay();
      }
    }

    this.canvas.requestRenderAll();
  }

  /* ----------------------------------------------------------------- history */

  private resetHistory(): void {
    this.history = [
      {
        json: this.canvas.toObject() as Record<string, unknown>,
        crop: this.cropBox ? { ...this.cropBox } : null,
        natural: { ...this.natural },
      },
    ];
    this.cursor = 0;
    this.notify();
  }

  private commit(): void {
    if (this.destroyed || this.restoring) return;
    this.history.length = this.cursor + 1;
    this.history.push({
      json: this.canvas.toObject() as Record<string, unknown>,
      crop: this.cropBox ? { ...this.cropBox } : null,
      natural: { ...this.natural },
    });
    while (this.history.length > MAX_HISTORY) this.history.shift();
    this.cursor = this.history.length - 1;
    this.notify();
  }

  private restore(): void {
    const entry = this.history[this.cursor];
    if (!entry) return;
    const json = JSON.parse(JSON.stringify(entry.json)) as Record<string, unknown>;
    const crop = entry.crop ? { ...entry.crop } : null;
    const natural = { ...entry.natural };
    this.restoring = true;
    this.pending = this.pending
      .then(() => this.applyEntry(json, crop, natural))
      .catch((error: unknown) => {
        this.restoring = false;
        console.error("Capicorn: could not restore canvas state", error);
      });
    this.notify();
  }

  private async applyEntry(
    json: Record<string, unknown>,
    crop: Box | null,
    natural: { width: number; height: number },
  ): Promise<void> {
    if (this.destroyed) return;
    const canvas = this.canvas;
    canvas.discardActiveObject();

    // The screenshot is not part of the serialized state, so a restore that
    // changes the pixel density has to rebuild it at the recorded size first.
    const width = Math.max(1, Math.round(natural.width));
    const height = Math.max(1, Math.round(natural.height));
    if (
      width !== Math.round(this.natural.width) ||
      height !== Math.round(this.natural.height)
    ) {
      const rebuilt = this.buildImage(width, height);
      if (rebuilt) {
        this.image = rebuilt.image;
        this.natural = { width, height };
        this.sourceDataUrl = rebuilt.dataUrl;
      }
    }

    await canvas.loadFromJSON(json);
    this.chrome.clear();

    // loadFromJSON clears everything, including the screenshot object.
    const image = this.image;
    if (image) {
      if (!canvas.getObjects().includes(image)) canvas.insertAt(0, image);
      // Re-fit at the restored natural size: annotations keep the coordinates
      // they were authored in (same size + same container ⇒ same display box).
      const box = this.imageBoxFor(canvas.getWidth(), canvas.getHeight());
      image.set({
        left: box.left,
        top: box.top,
        scaleX: box.width / this.natural.width,
        scaleY: box.height / this.natural.height,
      });
      image.setCoords();
    }

    this.cropBox = crop ? this.clampToImage(crop) : null;
    this.applyInteractivity();
    this.renderCropOverlay();
    canvas.requestRenderAll();
    this.restoring = false;
    this.notify();
  }

  /* ------------------------------------------------------------------ export */

  private outputBox(): Box {
    const crop = this.cropBox;
    if (crop) {
      const clamped = this.clampToImage(crop);
      if (clamped.width >= 1 && clamped.height >= 1) {
        return {
          left: Math.round(clamped.left),
          top: Math.round(clamped.top),
          width: Math.max(1, Math.round(clamped.width)),
          height: Math.max(1, Math.round(clamped.height)),
        };
      }
    }
    const image = this.imageBox();
    return {
      left: Math.round(image.left),
      top: Math.round(image.top),
      width: Math.max(1, Math.round(image.width)),
      height: Math.max(1, Math.round(image.height)),
    };
  }

  /** Export at (at least) the capture's native resolution. */
  private outputMultiplier(box: Box): number {
    const displayed = this.imageBox().width;
    if (displayed <= 0) return 1;
    const native = Math.min(Math.max(this.natural.width / displayed, 1), 4);
    const edge = Math.max(box.width, box.height);
    if (edge <= 0) return native;
    return Math.max(1, Math.min(native, MAX_EXPORT_EDGE / edge));
  }

  /**
   * Turn effect regions into real pixels on the flattened image layer, at
   * export resolution, in a fixed order: blur/redaction first, then the
   * highlighter tint, then the spotlight dimming (so the lit hole always shows
   * the finished region).
   */
  private applyEffects(
    layer: HTMLCanvasElement,
    effects: fabric.FabricObject[],
    box: Box,
    multiplier: number,
  ): void {
    if (!effects.length) return;
    const size = { width: layer.width, height: layer.height };
    const cache = new Map<fabric.FabricObject, DeviceRect>();
    const device = (object: fabric.FabricObject): DeviceRect => {
      const cached = cache.get(object);
      if (cached) return cached;
      const rect = toDevice(object.getBoundingRect(), box, multiplier, size);
      cache.set(object, rect);
      return rect;
    };

    const spotlights: DeviceRect[] = [];
    const lenses: Array<{ rect: DeviceRect; zoom: number }> = [];
    for (const object of effects) {
      const rect = device(object);
      if (isBlurRegion(object)) {
        // Radius is authored in scene px; scale it to the export pixel grid.
        applyBlurRegion(layer, rect, object.mode, Math.max(1, object.radius * multiplier));
      } else if (isHighlighterRegion(object)) {
        applyHighlighter(layer, rect, object.ink, object.alpha);
      } else if (isMagnifyRegion(object)) {
        lenses.push({ rect, zoom: object.zoom });
      } else if (isSpotlightRegion(object)) {
        spotlights.push(rect);
      }
    }

    if (spotlights.length) applySpotlight(layer, spotlights);

    // Lenses last: each one copies finished pixels under its box and blows them
    // up, so it must not pick up the still-live region objects or get dimmed by
    // a neighbouring spotlight border.
    for (const lens of lenses) drawMagnify(layer, lens.rect, lens.zoom);
  }

  /* ------------------------------------------------------------------ events */

  private notify(): void {
    this.container.dispatchEvent(
      new CustomEvent("capicorn:change", {
        bubbles: true,
        detail: {
          tool: this.tool,
          canUndo: this.canUndo(),
          canRedo: this.canRedo(),
        },
      }),
    );
  }
}