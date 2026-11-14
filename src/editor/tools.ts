// Capicorn annotation tools (Fabric.js 6).
//
// Each tool reacts to pointer input forwarded by Editor and mutates the canvas
// through a narrow ToolHost surface. Editor owns history, layout, export and
// interactivity; tools only draw/place objects and report completion.

import * as fabric from "fabric";
import {
  DEFAULT_BLUR_RADIUS,
  HIGHLIGHTER_ALPHA,
  SPOTLIGHT_DIM,
  clampBlurRadius,
  type BlurMode,
} from "./blur";
import type { ToolId } from "./editor";

export type { BlurMode } from "./blur";

export interface Point {
  x: number;
  y: number;
}

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Default accent colour (also used for the editor's crop chrome). */
export const ACCENT = "#ff3b30";
/** Default annotation stroke width. */
export const STROKE_WIDTH = 3;
/** Bounds accepted by Editor#setStrokeWidth. */
export const MIN_STROKE_WIDTH = 1;
export const MAX_STROKE_WIDTH = 12;
export const FONT_STACK =
  "Inter, 'Helvetica Neue', system-ui, -apple-system, sans-serif";
/** Pixelation block size, in canvas px, at 1x (scaled by the export multiplier). */
export const BLUR_BLOCK = 12;
/** Minimum drag distance before a shape is kept. */
const MIN_DRAG = 8;

/**
 * Region objects (blur / spotlight / highlighter) are Rectangles with their own
 * serialized type, so a restored canvas (undo/redo) still knows which live
 * objects are export-time effects rather than ink. Their tuning (blur mode and
 * radius, highlighter ink) rides along in the serialized JSON, so undo/redo and
 * layout changes never reset a region to the defaults.
 */
type RectOptions = Partial<fabric.RectProps>;

export class BlurRegion extends fabric.Rect {
  static type = "capicorn_blur";
  /** Keep the tuning in the serialized JSON (undo/redo, save/load). */
  static customProperties = ["mode", "radius"];
  mode: BlurMode = "gaussian";
  /** Gaussian radius / pixelate block size, in scene px. */
  radius = DEFAULT_BLUR_RADIUS;

  constructor(options: RectOptions & { mode?: BlurMode; radius?: number } = {}) {
    const { mode, radius, ...rect } = options;
    super(rect as RectOptions);
    if (mode === "gaussian" || mode === "pixelate" || mode === "blackout") {
      this.mode = mode;
    }
    if (typeof radius === "number" && Number.isFinite(radius)) {
      this.radius = clampBlurRadius(radius);
    }
  }
}
fabric.classRegistry.setClass(BlurRegion, "capicorn_blur");

export const isBlurRegion = (
  object: fabric.FabricObject,
): object is BlurRegion => object instanceof BlurRegion;

/**
 * Spotlight: dims everything OUTSIDE the region on export. The live preview
 * paints the same dimming itself (drawing outside its own bounds), so what you
 * drag is what you get.
 */
export class SpotlightRegion extends fabric.Rect {
  static type = "capicorn_spotlight";
  /** Outline thickness (user selected) survives undo/redo. */
  static customProperties = ["thickness"];
  thickness = 2;

  constructor(options: RectOptions & { thickness?: number } = {}) {
    const { thickness, ...rect } = options;
    super(rect as RectOptions);
    if (typeof thickness === "number" && Number.isFinite(thickness)) {
      this.thickness = Math.max(1, thickness);
    }
    // A cached object would clip the dimming to its own bounding box.
    this.objectCaching = false;
  }

  _render(context: CanvasRenderingContext2D): void {
    const width = this.width ?? 0;
    const height = this.height ?? 0;
    if (width <= 0 || height <= 0) return;
    const reach = 100000;

    context.save();
    context.fillStyle = SPOTLIGHT_DIM;
    context.beginPath();
    context.rect(-reach, -reach, width + reach * 2, height + reach * 2);
    context.rect(0, 0, width, height);
    context.fill("evenodd");
    context.restore();

    context.save();
    context.strokeStyle = "rgba(255, 255, 255, 0.9)";
    context.lineWidth = Math.max(1, Math.min(60, this.thickness || 1));
    context.setLineDash([6, 4]);
    context.strokeRect(0.5, 0.5, width - 1, height - 1);
    context.restore();
  }
}
fabric.classRegistry.setClass(SpotlightRegion, "capicorn_spotlight");

export const isSpotlightRegion = (
  object: fabric.FabricObject,
): object is SpotlightRegion => object instanceof SpotlightRegion;

/** Highlighter: translucent ink rectangle, multiplied into the pixels. */
export class HighlighterRegion extends fabric.Rect {
  static type = "capicorn_highlight";
  static customProperties = ["ink", "alpha", "thickness"];
  /** Colour baked in at creation, so later ink changes do not recolour it. */
  ink = "#ffe066";
  alpha = HIGHLIGHTER_ALPHA;
  /**
   * Outline thickness of the live preview, in scene px. Effect regions are
   * swapped for pixels on export, so this edge is editing chrome only (same
   * convention as the blur region's dashed outline).
   */
  thickness = 2;

  constructor(
    options: RectOptions & { ink?: string; alpha?: number; thickness?: number } = {},
  ) {
    const { ink, alpha, thickness, ...rect } = options;
    super(rect as RectOptions);
    if (typeof ink === "string" && ink.trim()) this.ink = ink.trim();
    if (typeof alpha === "number" && Number.isFinite(alpha)) {
      this.alpha = Math.min(Math.max(alpha, 0.05), 1);
    }
    if (typeof thickness === "number" && Number.isFinite(thickness)) {
      this.thickness = Math.max(1, thickness);
    }
    this.set({
      fill: this.ink,
      opacity: this.alpha,
      globalCompositeOperation: "multiply",
    });
  }
}
fabric.classRegistry.setClass(HighlighterRegion, "capicorn_highlight");

export const isHighlighterRegion = (
  object: fabric.FabricObject,
): object is HighlighterRegion => object instanceof HighlighterRegion;

/** The source screenshot as the canvas currently holds it. */
export interface ImageInfo {
  /** Original capture data URL (flood-fill samples its pixels). */
  dataUrl: string;
  /** Native pixel size of the capture. */
  natural: { width: number; height: number };
  /** Scene-space placement of the image object. */
  left: number;
  top: number;
  scaleX: number;
  scaleY: number;
}

/** Typography for new captions (user selected). */
export interface TextStyle {
  fontFamily: string;
  fontSize: number;
  fontWeight: "normal" | "bold";
  fontStyle: "normal" | "italic";
}

/** The slice of Editor a tool is allowed to touch. */
export interface ToolHost {
  readonly canvas: fabric.Canvas;
  /** Canvas scene bounds, used to clamp pointer positions. */
  bounds(): Box;
  /** The loaded screenshot, or null before `load()` resolves. */
  imageInfo(): ImageInfo | null;
  /** Active caption typography (user selected). */
  textStyle(): TextStyle;
  addObject(object: fabric.FabricObject): void;
  removeObject(object: fabric.FabricObject): void;
  /** Record the current canvas state in the undo stack. */
  commit(): void;
  setCrop(box: Box | null): void;
  nextStepNumber(): number;
  /** Active annotation ink colour (user selected). */
  color(): string;
  /** Active annotation stroke width (user selected). */
  strokeWidth(): number;
  /** Redaction mode for NEW blur regions. */
  blurMode(): BlurMode;
  /** Gaussian radius / pixelate block size for NEW blur regions, in scene px. */
  blurRadius(): number;
  /** Freehand stroke width for NEW brush strokes, in scene px. */
  brushWidth(): number;
  /** Diameter of a new numbered step badge, in scene px. */
  stepSize(): number;
  /** Caption font size inside a new callout, in scene px. */
  calloutFontSize(): number;
  /** Outline thickness for new spotlight/highlighter regions, in scene px. */
  regionThickness(): number;
  /** Magnification for a new lens. */
  magnifyZoom(): number;
}

export interface Tool {
  readonly id: ToolId;
  readonly cursor: string;
  activate(): void;
  deactivate(): void;
  onPointerDown(point: Point, event: fabric.TPointerEvent): void;
  onPointerMove(point: Point): void;
  onPointerUp(point: Point): void;
  /** Return true when the tool consumed the Escape key. */
  onEscape(): boolean;
  /** Cancel any in-progress interaction (tool switch, blur, undo). */
  cancel(): void;
}

/* ------------------------------------------------------------------ helpers */

export const normalizeBox = (a: Point, b: Point): Box => ({
  left: Math.min(a.x, b.x),
  top: Math.min(a.y, b.y),
  width: Math.abs(a.x - b.x),
  height: Math.abs(a.y - b.y),
});

export const clampPoint = (point: Point, bounds: Box): Point => ({
  x: Math.min(Math.max(point.x, bounds.left), bounds.left + bounds.width),
  y: Math.min(Math.max(point.y, bounds.top), bounds.top + bounds.height),
});

/** Fit `size` inside `frame`, keeping the aspect ratio, centred, optional padding. */
export const fitBox = (
  size: { width: number; height: number },
  frame: Box,
  padding = 0,
  allowUpscale = false,
): Box => {
  const availableWidth = Math.max(1, frame.width - padding * 2);
  const availableHeight = Math.max(1, frame.height - padding * 2);
  const raw = Math.min(
    availableWidth / Math.max(size.width, 1),
    availableHeight / Math.max(size.height, 1),
  );
  const scale = allowUpscale ? raw : Math.min(raw, 1);
  const width = Math.max(1, size.width * scale);
  const height = Math.max(1, size.height * scale);
  return {
    left: frame.left + (frame.width - width) / 2,
    top: frame.top + (frame.height - height) / 2,
    width,
    height,
  };
};

/* -------------------------------------------------------------------- tools */

class SelectTool implements Tool {
  readonly id: ToolId = "select";
  readonly cursor = "default";

  constructor(protected readonly host: ToolHost) {}

  activate(): void {
    this.host.canvas.discardActiveObject();
  }
  deactivate(): void {}
  cancel(): void {}
  onPointerDown(): void {}
  onPointerMove(): void {}
  onPointerUp(): void {}

  onEscape(): boolean {
    const canvas = this.host.canvas;
    if (!canvas.getActiveObject()) return false;
    canvas.discardActiveObject();
    canvas.requestRenderAll();
    return true;
  }
}

/** Shared drag-a-rectangle behaviour. */
abstract class DragTool implements Tool {
  abstract readonly id: ToolId;
  readonly cursor: string = "crosshair";
  protected start: Point | null = null;

  constructor(protected readonly host: ToolHost) {}

  activate(): void {}
  deactivate(): void {
    this.cancel();
  }

  cancel(): void {
    this.start = null;
    this.abort();
  }

  onPointerDown(point: Point): void {
    this.cancel();
    this.start = clampPoint(point, this.host.bounds());
    this.begin(this.start);
  }

  onPointerMove(point: Point): void {
    if (!this.start) return;
    this.update(this.start, clampPoint(point, this.host.bounds()));
    this.host.canvas.requestRenderAll();
  }

  onPointerUp(point: Point): void {
    if (!this.start) return;
    const end = clampPoint(point, this.host.bounds());
    this.update(this.start, end);
    const start = this.start;
    this.start = null;
    this.finish(start, end);
    this.host.canvas.requestRenderAll();
  }

  onEscape(): boolean {
    if (!this.start) return false;
    this.cancel();
    this.host.canvas.requestRenderAll();
    return true;
  }

  protected abstract begin(point: Point): void;
  protected abstract update(start: Point, point: Point): void;
  protected abstract finish(start: Point, end: Point): void;
  /** Drop any scratch object (called on cancel / tool switch). */
  protected abstract abort(): void;
}

class RectTool extends DragTool {
  readonly id: ToolId = "rect";
  private shape: fabric.Rect | null = null;

  protected begin(point: Point): void {
    const shape = new fabric.Rect({
      left: point.x,
      top: point.y,
      width: 1,
      height: 1,
      originX: "left",
      originY: "top",
      fill: "transparent",
      stroke: this.host.color(),
      strokeWidth: this.host.strokeWidth(),
      strokeUniform: true,
      selectable: false,
      evented: false,
      objectCaching: false,
    });
    this.shape = shape;
    this.host.addObject(shape);
  }

  protected update(start: Point, point: Point): void {
    const shape = this.shape;
    if (!shape) return;
    const box = normalizeBox(start, point);
    shape.set({
      left: box.left,
      top: box.top,
      width: Math.max(box.width, 1),
      height: Math.max(box.height, 1),
    });
    shape.setCoords();
  }

  protected finish(start: Point, end: Point): void {
    const shape = this.shape;
    this.shape = null;
    if (!shape) return;
    if (Math.abs(end.x - start.x) < MIN_DRAG || Math.abs(end.y - start.y) < MIN_DRAG) {
      this.host.removeObject(shape);
      return;
    }
    this.host.commit();
  }

  protected abort(): void {
    if (!this.shape) return;
    const shape = this.shape;
    this.shape = null;
    this.host.removeObject(shape);
  }
}

/** Blur-mode preview styling: the region itself never exports, only its effect. */
export const blurRegionPreview = (mode: BlurMode): { fill: string; stroke: string } => {
  switch (mode) {
    case "blackout":
      return { fill: "rgba(17, 24, 39, 0.88)", stroke: "rgba(17, 24, 39, 0.95)" };
    case "pixelate":
      return { fill: "rgba(17, 24, 39, 0.34)", stroke: "rgba(255, 255, 255, 0.85)" };
    case "gaussian":
    default:
      return { fill: "rgba(120, 150, 255, 0.22)", stroke: "rgba(255, 255, 255, 0.9)" };
  }
};

/**
 * Shared drag-a-region behaviour for the export-time effect tools: the object
 * lives on the canvas (movable, scalable, undoable) and is swapped for pixels
 * only when Editor flattens.
 */
abstract class RegionTool<T extends fabric.Rect> extends DragTool {
  private shape: T | null = null;

  protected abstract createRegion(point: Point): T;

  protected begin(point: Point): void {
    const shape = this.createRegion(point);
    this.shape = shape;
    this.host.addObject(shape);
  }

  protected update(start: Point, point: Point): void {
    const shape = this.shape;
    if (!shape) return;
    const box = normalizeBox(start, point);
    shape.set({
      left: box.left,
      top: box.top,
      width: Math.max(box.width, 1),
      height: Math.max(box.height, 1),
    });
    shape.setCoords();
  }

  protected finish(start: Point, end: Point): void {
    const shape = this.shape;
    this.shape = null;
    if (!shape) return;
    if (Math.abs(end.x - start.x) < MIN_DRAG || Math.abs(end.y - start.y) < MIN_DRAG) {
      this.host.removeObject(shape);
      return;
    }
    this.host.commit();
  }

  protected abort(): void {
    if (!this.shape) return;
    const shape = this.shape;
    this.shape = null;
    this.host.removeObject(shape);
  }
}

class BlurTool extends RegionTool<BlurRegion> {
  readonly id: ToolId = "blur";

  protected createRegion(point: Point): BlurRegion {
    const mode = this.host.blurMode();
    const preview = blurRegionPreview(mode);
    return new BlurRegion({
      left: point.x,
      top: point.y,
      width: 1,
      height: 1,
      originX: "left",
      originY: "top",
      mode,
      radius: this.host.blurRadius(),
      fill: preview.fill,
      stroke: preview.stroke,
      strokeWidth: 1,
      strokeDashArray: [6, 4],
      selectable: false,
      evented: false,
      objectCaching: false,
    });
  }
}

class SpotlightTool extends RegionTool<SpotlightRegion> {
  readonly id: ToolId = "spotlight";

  protected createRegion(point: Point): SpotlightRegion {
    const thickness = this.host.regionThickness();
    return new SpotlightRegion({
      left: point.x,
      top: point.y,
      width: 1,
      height: 1,
      originX: "left",
      originY: "top",
      thickness,
      // The dimming is painted by the region itself (see SpotlightRegion._render).
      fill: "transparent",
      stroke: "transparent",
      strokeWidth: 0,
      selectable: false,
      evented: false,
      objectCaching: false,
    });
  }
}

class HighlighterTool extends RegionTool<HighlighterRegion> {
  readonly id: ToolId = "highlighter";

  protected createRegion(point: Point): HighlighterRegion {
    const ink = this.host.color();
    const thickness = this.host.regionThickness();
    return new HighlighterRegion({
      left: point.x,
      top: point.y,
      width: 1,
      height: 1,
      originX: "left",
      originY: "top",
      ink,
      thickness,
      // Preview-only edge: the region itself never exports (only its tint does).
      stroke: ink,
      strokeWidth: thickness,
      strokeUniform: true,
      selectable: false,
      evented: false,
      objectCaching: false,
    });
  }
}

/** Solid filled arrow head + shaft, as one scalable Path object. */
const arrowPath = (
  start: Point,
  end: Point,
  thickness = STROKE_WIDTH * 2,
  headLength = 18,
  headWidth = 20,
): string => {
  const dx = end.x - start.x;
  const dy = end.y - start.y;
  const length = Math.hypot(dx, dy) || 1;
  const ux = dx / length;
  const uy = dy / length;
  const nx = -uy;
  const ny = ux;
  const halfShaft = Math.max(thickness, 2) / 2;
  const head = Math.min(headLength, length * 0.6);
  const halfHead = headWidth / 2;
  const baseX = end.x - ux * head;
  const baseY = end.y - uy * head;
  const points: Array<[number, number]> = [
    [start.x + nx * halfShaft, start.y + ny * halfShaft],
    [baseX + nx * halfShaft, baseY + ny * halfShaft],
    [baseX + nx * halfHead, baseY + ny * halfHead],
    [end.x, end.y],
    [baseX - nx * halfHead, baseY - ny * halfHead],
    [baseX - nx * halfShaft, baseY - ny * halfShaft],
    [start.x - nx * halfShaft, start.y - ny * halfShaft],
  ];
  const round = (value: number) => Math.round(value * 100) / 100;
  return (
    "M " +
    points.map(([x, y]) => `${round(x)} ${round(y)}`).join(" L ") +
    " Z"
  );
};

class ArrowTool extends DragTool {
  readonly id: ToolId = "arrow";
  private shaft: fabric.Line | null = null;
  private head: fabric.Triangle | null = null;

  protected begin(point: Point): void {
    const ink = this.host.color();
    const shaft = new fabric.Line([point.x, point.y, point.x + 1, point.y], {
      stroke: ink,
      strokeWidth: this.host.strokeWidth(),
      strokeLineCap: "round",
      selectable: false,
      evented: false,
      objectCaching: false,
    });
    const head = new fabric.Triangle({
      left: point.x,
      top: point.y,
      originX: "center",
      originY: "center",
      width: 18,
      height: 20,
      fill: ink,
      selectable: false,
      evented: false,
      objectCaching: false,
    });
    this.shaft = shaft;
    this.head = head;
    this.host.addObject(shaft);
    this.host.addObject(head);
  }

  protected update(start: Point, point: Point): void {
    const shaft = this.shaft;
    const head = this.head;
    if (!shaft || !head) return;
    const headLength = 18;
    const dx = point.x - start.x;
    const dy = point.y - start.y;
    const length = Math.hypot(dx, dy);
    const ux = length ? dx / length : 0;
    const uy = length ? dy / length : 0;
    // Stop the shaft short of the head so the stroke does not poke through it.
    const shaftEnd =
      length > headLength
        ? { x: point.x - ux * headLength * 0.9, y: point.y - uy * headLength * 0.9 }
        : point;
    shaft.set({ x1: start.x, y1: start.y, x2: shaftEnd.x, y2: shaftEnd.y });
    shaft.setCoords();
    const angle = (Math.atan2(dy, dx) * 180) / Math.PI + 90;
    head.set({ left: point.x, top: point.y, angle });
    head.setCoords();
  }

  protected finish(start: Point, end: Point): void {
    const shaft = this.shaft;
    const head = this.head;
    this.shaft = null;
    this.head = null;
    if (shaft) this.host.removeObject(shaft);
    if (head) this.host.removeObject(head);
    if (Math.hypot(end.x - start.x, end.y - start.y) < MIN_DRAG) return;
    const arrow = new fabric.Path(arrowPath(start, end, this.host.strokeWidth() * 2), {
      fill: this.host.color(),
      stroke: "transparent",
      strokeWidth: 0,
      selectable: false,
      evented: false,
      objectCaching: false,
    });
    this.host.addObject(arrow);
    this.host.commit();
  }

  protected abort(): void {
    const shaft = this.shaft;
    const head = this.head;
    this.shaft = null;
    this.head = null;
    if (shaft) this.host.removeObject(shaft);
    if (head) this.host.removeObject(head);
  }
}

class TextTool implements Tool {
  readonly id: ToolId = "text";
  readonly cursor = "text";
  private editing: fabric.IText | null = null;

  constructor(private readonly host: ToolHost) {}

  activate(): void {}
  deactivate(): void {
    this.cancel();
  }

  cancel(): void {
    const text = this.editing;
    this.editing = null;
    if (text && text.isEditing) text.exitEditing();
  }

  onPointerDown(point: Point): void {
    const canvas = this.host.canvas;
    const ink = this.host.color();
    const style = this.host.textStyle();
    const text = new fabric.IText("Text", {
      left: point.x,
      top: point.y,
      originX: "left",
      originY: "top",
      fontFamily: style.fontFamily || FONT_STACK,
      fontSize: style.fontSize,
      fontWeight: style.fontWeight,
      fontStyle: style.fontStyle,
      fill: ink,
      textBackgroundColor: "",
      padding: 4,
      borderColor: ink,
      editingBorderColor: ink,
      cursorColor: ink,
      selectable: false,
      evented: false,
    });
    this.editing = text;
    this.host.addObject(text);
    canvas.setActiveObject(text);
    text.enterEditing();
    // Select the placeholder so the first keystroke replaces it.
    text.selectAll();
    text.on("editing:exited", () => this.settle(text));
  }

  /** Drop the object when the caption was left empty, otherwise record history. */
  private settle(text: fabric.IText): void {
    if (this.editing === text) this.editing = null;
    if (!text.text || !text.text.trim()) {
      this.host.removeObject(text);
      this.host.canvas.requestRenderAll();
      return;
    }
    this.host.commit();
  }

  onPointerMove(): void {}
  onPointerUp(): void {}

  onEscape(): boolean {
    if (!this.editing) return false;
    this.cancel();
    return true;
  }
}

class StepTool implements Tool {
  readonly id: ToolId = "step";
  readonly cursor = "copy";

  constructor(private readonly host: ToolHost) {}

  activate(): void {}
  deactivate(): void {}
  cancel(): void {}
  onPointerMove(): void {}
  onPointerUp(): void {}
  onEscape(): boolean {
    return false;
  }

  onPointerDown(point: Point): void {
    const number = this.host.nextStepNumber();
    const diameter = Math.max(16, Math.min(64, this.host.stepSize()));
    const radius = diameter / 2;
    const badge = new fabric.Circle({
      radius,
      left: 0,
      top: 0,
      originX: "center",
      originY: "center",
      fill: this.host.color(),
      stroke: "#ffffff",
      strokeWidth: Math.max(2, Math.round(diameter * 0.07)),
      strokeUniform: true,
    });
    const label = new fabric.Text(String(number), {
      left: 0,
      top: 0,
      originX: "center",
      originY: "center",
      fontFamily: FONT_STACK,
      fontSize: Math.max(10, Math.round(diameter * 0.56)),
      fontWeight: "700",
      fill: "#ffffff",
    });
    const group = new fabric.Group([badge, label], {
      left: point.x,
      top: point.y,
      originX: "center",
      originY: "center",
      selectable: false,
      evented: false,
      objectCaching: false,
    });
    group.set({ left: point.x, top: point.y });
    group.setCoords();
    this.host.addObject(group);
    this.host.commit();
  }
}

/* --------------------------------------------------------------- callout */

/** Rounded callout defaults. */
const CALLOUT_PAD = 16;
const CALLOUT_RADIUS = 14;
const CALLOUT_MIN_WIDTH = 140;
const CALLOUT_MIN_HEIGHT = 72;
const CALLOUT_DEFAULT = { width: 240, height: 112 };
const CALLOUT_TAIL_WIDTH = 32;
const CALLOUT_TAIL_HEIGHT = 26;

/**
 * Speech-balloon style callout: rounded body + pointer tail + editable caption,
 * bundled into one Group so it moves, scales, undoes and exports as a unit.
 * Click places a default-sized callout; dragging sizes it.
 */
class CalloutTool extends DragTool {
  readonly id: ToolId = "callout";
  private preview: fabric.Rect | null = null;

  protected begin(point: Point): void {
    const preview = new fabric.Rect({
      left: point.x,
      top: point.y,
      width: 1,
      height: 1,
      originX: "left",
      originY: "top",
      rx: CALLOUT_RADIUS,
      ry: CALLOUT_RADIUS,
      fill: "rgba(255, 255, 255, 0.22)",
      stroke: this.host.color(),
      strokeWidth: 1,
      strokeDashArray: [8, 5],
      selectable: false,
      evented: false,
      objectCaching: false,
    });
    this.preview = preview;
    this.host.addObject(preview);
  }

  protected update(start: Point, point: Point): void {
    const preview = this.preview;
    if (!preview) return;
    const box = normalizeBox(start, point);
    preview.set({
      left: box.left,
      top: box.top,
      width: Math.max(box.width, 1),
      height: Math.max(box.height, 1),
    });
    preview.setCoords();
  }

  protected finish(start: Point, end: Point): void {
    this.abort();
    const box = normalizeBox(start, end);
    const small = box.width < MIN_DRAG || box.height < MIN_DRAG;
    const left = Math.min(box.left, start.x);
    const top = Math.min(box.top, start.y);
    const width = small ? CALLOUT_DEFAULT.width : Math.max(box.width, CALLOUT_MIN_WIDTH);
    const height = small ? CALLOUT_DEFAULT.height : Math.max(box.height, CALLOUT_MIN_HEIGHT);

    const { group, text } = this.build(left, top, width, height);
    this.host.addObject(group);
    group.setCoords();
    this.host.commit();

    // Drop straight into text editing; if the engine refuses, the user can
    // still double-click the caption with the select tool.
    const canvas = this.host.canvas;
    try {
      canvas.setActiveObject(group);
      text.enterEditing();
      text.selectAll();
      canvas.requestRenderAll();
    } catch {
      canvas.requestRenderAll();
    }
  }

  protected abort(): void {
    if (!this.preview) return;
    const preview = this.preview;
    this.preview = null;
    this.host.removeObject(preview);
  }

  private build(
    left: number,
    top: number,
    width: number,
    height: number,
  ): { group: fabric.Group; text: fabric.IText } {
    const ink = this.host.color();
    const style = this.host.textStyle();
    // The callout has its own size control (12..120), independent of captions;
    // still capped so a long word cannot overrun the balloon.
    const fontSize = Math.max(
      12,
      Math.min(120, Math.round(this.host.calloutFontSize()), Math.round(width / 4)),
    );

    // Tail first so the body's fill hides the seam where they overlap.
    const tail = new fabric.Triangle({
      left: CALLOUT_PAD + CALLOUT_TAIL_WIDTH / 2,
      top: height + CALLOUT_TAIL_HEIGHT / 2 - 2,
      originX: "center",
      originY: "center",
      width: CALLOUT_TAIL_WIDTH,
      height: CALLOUT_TAIL_HEIGHT,
      angle: 180,
      fill: "#ffffff",
      stroke: ink,
      strokeWidth: 2,
      strokeUniform: true,
      selectable: false,
      evented: false,
    });

    const body = new fabric.Rect({
      left: 0,
      top: 0,
      originX: "left",
      originY: "top",
      width,
      height,
      rx: CALLOUT_RADIUS,
      ry: CALLOUT_RADIUS,
      fill: "#ffffff",
      stroke: ink,
      strokeWidth: 2,
      strokeUniform: true,
      selectable: false,
      evented: false,
    });

    const text = new fabric.IText("Callout", {
      left: CALLOUT_PAD,
      top: CALLOUT_PAD,
      originX: "left",
      originY: "top",
      width: Math.max(40, width - CALLOUT_PAD * 2),
      fontFamily: style.fontFamily || FONT_STACK,
      fontSize,
      fontWeight: style.fontWeight,
      fontStyle: style.fontStyle,
      fill: ink,
      textBackgroundColor: "",
      borderColor: ink,
      editingBorderColor: ink,
      cursorColor: ink,
      selectable: false,
      evented: false,
    });

    const group = new fabric.Group([tail, body, text], {
      subTargetCheck: true,
      interactive: true,
      objectCaching: false,
      selectable: false,
      evented: false,
      hoverCursor: "move",
      borderColor: ink,
    });
    group.set({ left, top });
    group.setCoords();
    return { group, text };
  }
}

class CropTool extends DragTool {
  readonly id: ToolId = "crop";
  private frame: fabric.Rect | null = null;

  protected begin(point: Point): void {
    const frame = new fabric.Rect({
      left: point.x,
      top: point.y,
      width: 1,
      height: 1,
      originX: "left",
      originY: "top",
      fill: "rgba(255, 255, 255, 0.08)",
      stroke: ACCENT,
      strokeWidth: 1,
      strokeDashArray: [8, 5],
      excludeFromExport: true,
      selectable: false,
      evented: false,
      objectCaching: false,
    });
    this.frame = frame;
    this.host.addObject(frame);
  }

  protected update(start: Point, point: Point): void {
    const frame = this.frame;
    if (!frame) return;
    const box = normalizeBox(start, point);
    frame.set({
      left: box.left,
      top: box.top,
      width: Math.max(box.width, 1),
      height: Math.max(box.height, 1),
    });
    frame.setCoords();
  }

  protected finish(start: Point, end: Point): void {
    this.abort();
    const box = normalizeBox(start, end);
    // A plain click resets the crop back to the whole capture.
    if (box.width < MIN_DRAG || box.height < MIN_DRAG) {
      this.host.setCrop(null);
      return;
    }
    this.host.setCrop(box);
  }

  protected abort(): void {
    if (!this.frame) return;
    const frame = this.frame;
    this.frame = null;
    this.host.removeObject(frame);
  }
}

/* ----------------------------------------------------------------- bucket */

/** Per-channel tolerance when matching the seed colour. */
const FILL_TOLERANCE = 32;
/** Hard ceiling on flood-fill steps relative to the image area. */
const FILL_WORK_FACTOR = 4;

/** Decode a data URL into an HTMLImageElement. */
const loadImageElement = (url: string): Promise<HTMLImageElement> =>
  new Promise((resolve, reject) => {
    const element = new Image();
    element.onload = () => resolve(element);
    element.onerror = () => reject(new Error("Capicorn: could not decode image"));
    element.src = url;
  });

/** Resolve any CSS colour to its RGB channels via a 1x1 canvas. */
const colorChannels = (color: string): [number, number, number] => {
  const probe = document.createElement("canvas");
  probe.width = 1;
  probe.height = 1;
  const context = probe.getContext("2d");
  if (!context) return [255, 59, 48];
  context.fillStyle = "#000000";
  context.fillStyle = color;
  context.fillRect(0, 0, 1, 1);
  const [r, g, b] = context.getImageData(0, 0, 1, 1).data;
  return [r, g, b];
};

/**
 * Paint-bucket: flood-fills the region under the pointer inside the screenshot
 * and drops the result on the canvas as a normal (exportable, rescalable)
 * overlay image, so it stacks, undoes and flattens like every other annotation.
 *
 * Non-destructive: the source capture is never mutated — the fill is sampled
 * from a decoded copy and only the overlay object is added.
 */
class BucketTool implements Tool {
  readonly id: ToolId = "bucket";
  readonly cursor = "crosshair";

  /** Decoded source pixels, kept only while the tool is active. */
  private source: { url: string; data: ImageData } | null = null;
  /** Bumped on every fill/cancel so late async fills can bail out. */
  private generation = 0;
  private busy = false;

  constructor(private readonly host: ToolHost) {}

  activate(): void {
    // Warm the pixel cache so the first click feels instant.
    void this.pixels(this.host.imageInfo());
  }

  deactivate(): void {
    this.cancel();
  }

  cancel(): void {
    this.generation += 1;
    this.busy = false;
    this.source = null;
  }

  onPointerDown(): void {}
  onPointerMove(): void {}

  onEscape(): boolean {
    return false;
  }

  onPointerUp(point: Point): void {
    const info = this.host.imageInfo();
    if (!info || this.busy) return;
    const scaleX = info.scaleX || 1;
    const scaleY = info.scaleY || 1;
    const px = (point.x - info.left) / scaleX;
    const py = (point.y - info.top) / scaleY;
    // Ignore clicks in the letterbox around the screenshot.
    if (px < 0 || py < 0) return;
    if (px >= info.natural.width || py >= info.natural.height) return;

    const seed = { x: Math.floor(px), y: Math.floor(py) };
    const token = ++this.generation;
    this.busy = true;
    void this.fill(info, seed, token);
  }

  private async fill(
    info: ImageInfo,
    seed: { x: number; y: number },
    token: number,
  ): Promise<void> {
    try {
      const pixels = await this.pixels(info);
      if (!pixels || token !== this.generation) return;

      const width = pixels.width;
      const height = pixels.height;
      const overlay = this.paint(pixels, seed, width, height);
      if (!overlay || token !== this.generation) return;

      const object = new fabric.FabricImage(overlay, {
        left: info.left,
        top: info.top,
        originX: "left",
        originY: "top",
        scaleX: info.scaleX || 1,
        scaleY: info.scaleY || 1,
        selectable: false,
        evented: false,
        objectCaching: false,
        // The overlay covers the whole capture; only its painted pixels may
        // intercept clicks in select mode, or it would hide the annotations below.
        perPixelTargetFind: true,
      });
      object.setCoords();
      this.host.addObject(object);
      this.host.commit();
      this.host.canvas.requestRenderAll();
    } finally {
      if (token === this.generation) this.busy = false;
    }
  }

  /** Decode (and cache) the capture at its natural size. */
  private async pixels(info: ImageInfo | null): Promise<ImageData | null> {
    if (!info) return null;
    if (this.source && this.source.url === info.dataUrl) return this.source.data;
    const url = info.dataUrl;
    try {
      const element = await loadImageElement(url);
      const width = Math.max(1, Math.floor(info.natural.width));
      const height = Math.max(1, Math.floor(info.natural.height));
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      if (!context) return null;
      context.drawImage(element, 0, 0, width, height);
      const data = context.getImageData(0, 0, width, height);
      this.source = { url, data };
      return data;
    } catch {
      return null;
    }
  }

  /**
   * Scanline flood fill from `seed`, returned as a natural-size canvas whose
   * filled pixels carry the active ink colour at full alpha and are transparent
   * everywhere else.
   */
  private paint(
    pixels: ImageData,
    seed: { x: number; y: number },
    width: number,
    height: number,
  ): HTMLCanvasElement | null {
    const source = pixels.data;
    const [r, g, b] = colorChannels(this.host.color());
    const mask = document.createElement("canvas");
    mask.width = width;
    mask.height = height;
    const maskContext = mask.getContext("2d");
    if (!maskContext) return null;
    const image = maskContext.createImageData(width, height);
    const out = image.data;

    const visited = new Uint8Array(width * height);
    const seedIndex = seed.y * width + seed.x;
    const seedOffset = seedIndex * 4;
    const sr = source[seedOffset];
    const sg = source[seedOffset + 1];
    const sb = source[seedOffset + 2];
    const sa = source[seedOffset + 3];
    const tolerance = FILL_TOLERANCE;

    const matches = (index: number): boolean => {
      const offset = index * 4;
      return (
        Math.abs(source[offset] - sr) <= tolerance &&
        Math.abs(source[offset + 1] - sg) <= tolerance &&
        Math.abs(source[offset + 2] - sb) <= tolerance &&
        Math.abs(source[offset + 3] - sa) <= tolerance
      );
    };

    const limit = width * height * FILL_WORK_FACTOR;
    const stack: number[] = [seed.x, seed.y];
    let work = 0;
    let filled = 0;
    let exhausted = false;

    while (stack.length) {
      const y = stack.pop() as number;
      let x = stack.pop() as number;
      let index = y * width + x;
      if (visited[index] || !matches(index)) continue;

      while (x > 0 && !visited[index - 1] && matches(index - 1)) {
        x -= 1;
        index -= 1;
      }

      let spanAbove = false;
      let spanBelow = false;
      while (x < width && !visited[index] && matches(index)) {
        visited[index] = 1;
        const offset = index * 4;
        out[offset] = r;
        out[offset + 1] = g;
        out[offset + 2] = b;
        out[offset + 3] = 255;
        filled += 1;
        work += 1;

        if (y > 0) {
          const above = index - width;
          const open = !visited[above] && matches(above);
          if (open && !spanAbove) {
            stack.push(x, y - 1);
            spanAbove = true;
          } else if (!open) {
            spanAbove = false;
          }
        }
        if (y < height - 1) {
          const below = index + width;
          const open = !visited[below] && matches(below);
          if (open && !spanBelow) {
            stack.push(x, y + 1);
            spanBelow = true;
          } else if (!open) {
            spanBelow = false;
          }
        }

        x += 1;
        index += 1;
        if (work >= limit) {
          exhausted = true;
          break;
        }
      }
      if (exhausted) break;
    }

    if (!filled) return null;

    maskContext.putImageData(image, 0, 0);
    const overlay = document.createElement("canvas");
    overlay.width = width;
    overlay.height = height;
    const context = overlay.getContext("2d");
    if (!context) return null;
    context.fillStyle = this.host.color();
    context.fillRect(0, 0, width, height);
    context.globalCompositeOperation = "destination-in";
    context.drawImage(mask, 0, 0);
    context.globalCompositeOperation = "source-over";
    return overlay;
  }
}

/* ---------------------------------------------------------------- magnify */

/** Default magnification for a lens. */
export const MAGNIFY_ZOOM = 2;
const MIN_MAGNIFY_ZOOM = 1.2;
const MAX_MAGNIFY_ZOOM = 6;

export const clampMagnifyZoom = (value: number): number => {
  if (typeof value !== "number" || !Number.isFinite(value)) return MAGNIFY_ZOOM;
  const clamped = Math.min(Math.max(value, MIN_MAGNIFY_ZOOM), MAX_MAGNIFY_ZOOM);
  return Math.round(clamped * 10) / 10;
};

/** A rectangle in flattened-layer pixels (same shape as blur.ts DeviceRect). */
interface LensRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** Rounded-rectangle path (no ctx.roundRect: WebKitGTK support is patchy). */
const lensPath = (
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

/** Lens frame + zoom badge, shared by the live preview and the flattened lens. */
const drawLensChrome = (
  context: CanvasRenderingContext2D,
  x: number,
  y: number,
  width: number,
  height: number,
  zoom: number,
): void => {
  const radius = Math.min(width, height) * 0.12;
  const shortest = Math.min(width, height);
  const border = Math.max(2, Math.min(6, Math.round(shortest * 0.02)));

  context.save();
  // White halo first, so the lens reads on any background.
  context.beginPath();
  lensPath(context, x, y, width, height, radius);
  context.lineWidth = border + 2;
  context.strokeStyle = "rgba(255, 255, 255, 0.92)";
  context.stroke();
  context.beginPath();
  lensPath(context, x, y, width, height, radius);
  context.lineWidth = border;
  context.strokeStyle = ACCENT;
  context.stroke();

  const label = `${zoom % 1 === 0 ? zoom.toFixed(0) : zoom.toFixed(1)}×`;
  const fontSize = Math.max(
    9,
    Math.min(Math.round(shortest * 0.16), Math.round(width * 0.3), 40),
  );
  context.font = `600 ${fontSize}px ${FONT_STACK}`;
  context.textBaseline = "middle";
  const padding = Math.max(3, Math.round(fontSize * 0.34));
  const textWidth = context.measureText(label).width;
  const badgeWidth = textWidth + padding * 2;
  const badgeHeight = fontSize + padding * 1.6;
  const badgeX = x + border + 3;
  const badgeY = y + border + 3;
  context.beginPath();
  lensPath(context, badgeX, badgeY, badgeWidth, badgeHeight, badgeHeight / 2);
  context.fillStyle = "rgba(12, 16, 24, 0.82)";
  context.fill();
  context.fillStyle = "#ffffff";
  context.fillText(label, badgeX + padding, badgeY + badgeHeight / 2 + 0.5);
  context.restore();
};

/**
 * Paint the magnifier at export resolution: copy the pixels under `rect`, blow
 * them up `zoom`x about the lens centre, clip to a rounded box and frame it.
 * Editor calls this on the flattened IMAGE layer, so the lens shows real
 * screenshot pixels (never the annotations drawn over it).
 */
export const drawMagnify = (
  layer: HTMLCanvasElement,
  rect: LensRect,
  zoom: number,
): void => {
  const width = Math.floor(rect.width);
  const height = Math.floor(rect.height);
  if (width < 8 || height < 8) return;
  const x = Math.round(rect.x);
  const y = Math.round(rect.y);
  const context = layer.getContext("2d");
  if (!context) return;

  const patch = document.createElement("canvas");
  patch.width = width;
  patch.height = height;
  const patchContext = patch.getContext("2d");
  if (!patchContext) return;
  patchContext.drawImage(layer, x, y, width, height, 0, 0, width, height);

  const scale = clampMagnifyZoom(zoom);
  const radius = Math.min(width, height) * 0.12;

  context.save();
  context.beginPath();
  lensPath(context, x, y, width, height, radius);
  context.clip();
  context.fillStyle = "#ffffff";
  context.fillRect(x, y, width, height);
  context.imageSmoothingEnabled = true;
  context.imageSmoothingQuality = "high";
  context.drawImage(
    patch,
    x - (width * (scale - 1)) / 2,
    y - (height * (scale - 1)) / 2,
    width * scale,
    height * scale,
  );
  context.restore();

  drawLensChrome(context, x, y, width, height, scale);
};

/**
 * Magnifier lens. `zoom` rides in the serialized JSON so undo/redo keeps it.
 *
 * The live object paints tinted glass + frame only: the enlarged patch needs the
 * flattened screenshot pixels, and Editor draws those at export time
 * (`drawMagnify`). The box itself is the lens aperture, so it can be moved and
 * scaled like any other region and always magnifies what sits under it.
 */
export class MagnifyRegion extends fabric.Rect {
  static type = "capicorn_magnify";
  static customProperties = ["zoom"];
  zoom = MAGNIFY_ZOOM;

  constructor(options: RectOptions & { zoom?: number } = {}) {
    const { zoom, ...rect } = options;
    super(rect as RectOptions);
    if (typeof zoom === "number" && Number.isFinite(zoom)) {
      this.zoom = clampMagnifyZoom(zoom);
    }
    // All painting happens in _render / at export; the Rect itself draws nothing.
    this.set({ fill: "transparent", stroke: "transparent", strokeWidth: 0 });
    this.objectCaching = false;
  }

  _render(context: CanvasRenderingContext2D): void {
    const width = this.width ?? 0;
    const height = this.height ?? 0;
    if (width <= 0 || height <= 0) return;
    const radius = Math.min(width, height) * 0.12;

    context.save();
    context.beginPath();
    lensPath(context, 0, 0, width, height, radius);
    context.clip();
    context.fillStyle = "rgba(12, 16, 24, 0.28)";
    context.fillRect(0, 0, width, height);
    context.restore();

    drawLensChrome(context, 0, 0, width, height, this.zoom);
  }
}
fabric.classRegistry.setClass(MagnifyRegion, "capicorn_magnify");

export const isMagnifyRegion = (
  object: fabric.FabricObject,
): object is MagnifyRegion => object instanceof MagnifyRegion;

class MagnifyTool extends RegionTool<MagnifyRegion> {
  readonly id: ToolId = "magnify";
  readonly cursor = "zoom-in";

  protected createRegion(point: Point): MagnifyRegion {
    return new MagnifyRegion({
      left: point.x,
      top: point.y,
      width: 1,
      height: 1,
      originX: "left",
      originY: "top",
      zoom: this.host.magnifyZoom(),
      selectable: false,
      evented: false,
      objectCaching: false,
    });
  }
}

/* ------------------------------------------------------------------- brush */

/** Minimum pointer travel (scene px) before another sample point is kept. */
const BRUSH_SAMPLE = 1.5;
/** Cap on samples per stroke — keeps the path string (and export) sane. */
const BRUSH_MAX_POINTS = 1200;

/**
 * Freehand path with quadratic smoothing: each sample becomes a control point
 * and the curve runs through the midpoints between samples, so the result is
 * smooth without the overshoot a spline would add.
 */
export const smoothPathData = (points: Point[]): string => {
  if (!points.length) return "";
  const round = (value: number): number => Math.round(value * 100) / 100;
  if (points.length === 1) {
    const only = points[0];
    // A tap still marks: a zero-length round-capped segment is a dot.
    return `M ${round(only.x)} ${round(only.y)} L ${round(only.x + 0.01)} ${round(only.y)}`;
  }
  const first = points[0];
  let data = `M ${round(first.x)} ${round(first.y)}`;
  for (let index = 1; index < points.length - 1; index += 1) {
    const control = points[index];
    const next = points[index + 1];
    const midX = (control.x + next.x) / 2;
    const midY = (control.y + next.y) / 2;
    data += ` Q ${round(control.x)} ${round(control.y)} ${round(midX)} ${round(midY)}`;
  }
  const last = points[points.length - 1];
  data += ` L ${round(last.x)} ${round(last.y)}`;
  return data;
};

/**
 * Freehand pen: samples the pointer, previews the smoothed stroke live and
 * commits ONE fabric.Path on release (selectable, movable, undoable, exported
 * by flatten() like any other annotation).
 */
class BrushTool implements Tool {
  readonly id: ToolId = "brush";
  readonly cursor = "crosshair";

  private points: Point[] = [];
  /** The live preview / final stroke (same object, rebuilt as points arrive). */
  private stroke: fabric.Path | null = null;
  private drawing = false;

  constructor(private readonly host: ToolHost) {}

  activate(): void {}
  deactivate(): void {
    this.cancel();
  }

  cancel(): void {
    this.drawing = false;
    this.points = [];
    const stroke = this.stroke;
    this.stroke = null;
    if (stroke) this.host.removeObject(stroke);
  }

  onPointerDown(point: Point): void {
    this.cancel();
    this.drawing = true;
    this.points = [clampPoint(point, this.host.bounds())];
    this.paint();
  }

  onPointerMove(point: Point): void {
    if (!this.drawing) return;
    const next = clampPoint(point, this.host.bounds());
    const last = this.points[this.points.length - 1];
    if (!last) return;
    if (Math.hypot(next.x - last.x, next.y - last.y) < BRUSH_SAMPLE) return;
    if (this.points.length >= BRUSH_MAX_POINTS) return;
    this.points.push(next);
    this.paint();
  }

  onPointerUp(point: Point): void {
    if (!this.drawing) return;
    const end = clampPoint(point, this.host.bounds());
    const last = this.points[this.points.length - 1];
    if (last && Math.hypot(end.x - last.x, end.y - last.y) >= BRUSH_SAMPLE) {
      this.points.push(end);
    }
    this.drawing = false;
    this.paint();

    const stroke = this.stroke;
    this.stroke = null;
    this.points = [];
    if (!stroke) return;
    this.host.commit();
    this.host.canvas.requestRenderAll();
  }

  onEscape(): boolean {
    if (!this.drawing) return false;
    this.cancel();
    this.host.canvas.requestRenderAll();
    return true;
  }

  /** Rebuild the preview path from the current samples. */
  private paint(): void {
    const data = smoothPathData(this.points);
    const previous = this.stroke;
    this.stroke = null;
    if (previous) this.host.removeObject(previous);
    if (!data) return;

    const stroke = new fabric.Path(data, {
      fill: "transparent",
      stroke: this.host.color(),
      strokeWidth: this.host.brushWidth(),
      strokeLineCap: "round",
      strokeLineJoin: "round",
      strokeUniform: true,
      paintFirst: "stroke",
      selectable: false,
      evented: false,
      objectCaching: false,
    });
    this.stroke = stroke;
    this.host.addObject(stroke);
    this.host.canvas.requestRenderAll();
  }
}

export const createTool = (id: ToolId, host: ToolHost): Tool => {
  switch (id) {
    case "brush":
      return new BrushTool(host);
    case "magnify":
      return new MagnifyTool(host);
    case "arrow":
      return new ArrowTool(host);
    case "rect":
      return new RectTool(host);
    case "blur":
      return new BlurTool(host);
    case "spotlight":
      return new SpotlightTool(host);
    case "highlighter":
      return new HighlighterTool(host);
    case "callout":
      return new CalloutTool(host);
    case "text":
      return new TextTool(host);
    case "crop":
      return new CropTool(host);
    case "step":
      return new StepTool(host);
    case "bucket":
      return new BucketTool(host);
    case "select":
    default:
      return new SelectTool(host);
  }
};