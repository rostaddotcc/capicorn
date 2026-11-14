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
  ACCENT,
  BLUR_BLOCK,
  type Box,
  type Point,
  type Tool,
  type ToolHost,
  createTool,
  fitBox,
  isBlurRegion,
} from "./tools";

export type ToolId = "select" | "crop" | "arrow" | "text" | "rect" | "blur" | "step";

const TOOL_IDS: readonly ToolId[] = [
  "select",
  "crop",
  "arrow",
  "text",
  "rect",
  "blur",
  "step",
];

const MAX_HISTORY = 80;
/** Breathing room around the screenshot inside the canvas. */
const PADDING = 16;
/** Smallest useful crop / annotation. */
const MIN_SIZE = 8;
/** Never export more than this on the long side. */
const MAX_EXPORT_EDGE = 8192;

interface HistoryEntry {
  json: Record<string, unknown>;
  crop: Box | null;
}

const isToolId = (value: string): value is ToolId =>
  (TOOL_IDS as readonly string[]).includes(value);

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

  private tool: ToolId = "select";
  private cropBox: Box | null = null;

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
      addObject: (object) => this.addObject(object),
      removeObject: (object) => this.removeObject(object),
      commit: () => this.commit(),
      setCrop: (box) => this.setCrop(box),
      nextStepNumber: () => this.nextStepNumber(),
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

    const blurRegions = this.canvas.getObjects().filter(isBlurRegion);
    const hidden: fabric.FabricObject[] = [...blurRegions, ...this.chrome];
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

    this.pixelate(imageLayer, blurRegions, box, multiplier);

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
    this.restoring = true;
    this.pending = this.pending
      .then(() => this.applyEntry(json, crop))
      .catch((error: unknown) => {
        this.restoring = false;
        console.error("Capicorn: could not restore canvas state", error);
      });
    this.notify();
  }

  private async applyEntry(json: Record<string, unknown>, crop: Box | null): Promise<void> {
    if (this.destroyed) return;
    const canvas = this.canvas;
    canvas.discardActiveObject();

    await canvas.loadFromJSON(json);
    this.chrome.clear();

    // loadFromJSON clears everything, including the screenshot object.
    if (this.image && !canvas.getObjects().includes(this.image)) {
      canvas.insertAt(0, this.image);
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

  private pixelate(
    layer: HTMLCanvasElement,
    regions: fabric.FabricObject[],
    box: Box,
    multiplier: number,
  ): void {
    if (!regions.length) return;
    const context = layer.getContext("2d");
    if (!context) return;
    const block = Math.max(4, Math.round(BLUR_BLOCK * multiplier));

    for (const region of regions) {
      const area = region.getBoundingRect();
      const sx = Math.round((area.left - box.left) * multiplier);
      const sy = Math.round((area.top - box.top) * multiplier);
      const sw = Math.round(area.width * multiplier);
      const sh = Math.round(area.height * multiplier);
      if (sw < 2 || sh < 2) continue;

      const scratch = document.createElement("canvas");
      scratch.width = Math.max(1, Math.round(sw / block));
      scratch.height = Math.max(1, Math.round(sh / block));
      const scratchContext = scratch.getContext("2d");
      if (!scratchContext) continue;

      scratchContext.imageSmoothingEnabled = true;
      scratchContext.drawImage(layer, sx, sy, sw, sh, 0, 0, scratch.width, scratch.height);

      context.save();
      context.imageSmoothingEnabled = false;
      context.drawImage(scratch, 0, 0, scratch.width, scratch.height, sx, sy, sw, sh);
      context.restore();
    }
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