// Capicorn annotation tools (Fabric.js 6).
//
// Each tool reacts to pointer input forwarded by Editor and mutates the canvas
// through a narrow ToolHost surface. Editor owns history, layout, export and
// interactivity; tools only draw/place objects and report completion.

import * as fabric from "fabric";
import type { ToolId } from "./editor";

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
 * Blur regions are Rectangles with their own serialized type, so a restored
 * canvas (undo/redo) still knows which rectangles must be pixelated on export.
 */
export class BlurRegion extends fabric.Rect {
  static type = "capicorn_blur";
}
fabric.classRegistry.setClass(BlurRegion, "capicorn_blur");

export const isBlurRegion = (
  object: fabric.FabricObject,
): object is BlurRegion => object instanceof BlurRegion;

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
  readonly cursor = "crosshair";
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

class BlurTool extends DragTool {
  readonly id: ToolId = "blur";
  private shape: BlurRegion | null = null;

  protected begin(point: Point): void {
    const shape = new BlurRegion({
      left: point.x,
      top: point.y,
      width: 1,
      height: 1,
      originX: "left",
      originY: "top",
      fill: "rgba(17, 24, 39, 0.28)",
      stroke: "rgba(255, 255, 255, 0.85)",
      strokeWidth: 1,
      strokeDashArray: [6, 4],
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
    const radius = 15;
    const badge = new fabric.Circle({
      radius,
      left: 0,
      top: 0,
      originX: "center",
      originY: "center",
      fill: this.host.color(),
      stroke: "#ffffff",
      strokeWidth: 2,
      strokeUniform: true,
    });
    const label = new fabric.Text(String(number), {
      left: 0,
      top: 1,
      originX: "center",
      originY: "center",
      fontFamily: FONT_STACK,
      fontSize: 17,
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

export const createTool = (id: ToolId, host: ToolHost): Tool => {
  switch (id) {
    case "arrow":
      return new ArrowTool(host);
    case "rect":
      return new RectTool(host);
    case "blur":
      return new BlurTool(host);
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