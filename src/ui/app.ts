// Capicorn app shell — top bar, annotation toolbar, editor stage, status line.
// Talks to the locked Editor API and the locked Tauri command set only.
import { invoke } from "@tauri-apps/api/core";
import { Editor, type ToolId } from "../editor/editor";
import { cropDataUrl, selectRegion } from "./region";

type CaptureResult = { dataUrl: string; width: number; height: number };
type StatusKind = "info" | "success" | "error";
type CaptureMode = "full" | "region";

/** Additive Editor API (Wave 2, contract-locked). */
type InkApi = {
  setColor(hex: string): void;
  getColor(): string;
  setStrokeWidth(width: number): void;
  getStrokeWidth(): number;
};
type InkEditor = Editor & InkApi;

type ChangeDetail = { tool?: ToolId; canUndo?: boolean; canRedo?: boolean };

const icon = (body: string, size = 16): string =>
  `<svg viewBox="0 0 16 16" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;

const ICONS = {
  capture: icon(
    `<rect x="2.4" y="3" width="11.2" height="7.6" rx="1.5"/><path d="M6 13.4h4M8 10.6v2.8"/>`,
  ),
  caret: icon(`<path d="M4.6 6.4 8 9.8l3.4-3.4"/>`, 12),
  full: icon(`<rect x="2.2" y="2.8" width="11.6" height="8.4" rx="1.4"/><path d="M5.4 13.4h5.2"/>`),
  region: icon(
    `<path d="M2.6 6V3.6A1 1 0 0 1 3.6 2.6H6M10 2.6h2.4a1 1 0 0 1 1 1V6M13.4 10v2.4a1 1 0 0 1-1 1H10M6 13.4H3.6a1 1 0 0 1-1-1V10"/>`,
  ),
  check: icon(`<path d="M3.2 8.4 6.6 11.8l6.2-7"/>`, 12),
  undo: icon(`<path d="M6.2 4.2 3.4 7l2.8 2.8"/><path d="M3.6 7h5.2a3.6 3.6 0 0 1 0 7.2H6.4"/>`),
  redo: icon(`<path d="M9.8 4.2 12.6 7l-2.8 2.8"/><path d="M12.4 7H7.2a3.6 3.6 0 0 0 0 7.2h2.4"/>`),
  copy: icon(
    `<rect x="5.5" y="5.5" width="7.5" height="7.5" rx="1.6"/><path d="M10.5 5.5V4.1A1.6 1.6 0 0 0 8.9 2.5H4.1A1.6 1.6 0 0 0 2.5 4.1v4.8a1.6 1.6 0 0 0 1.6 1.6h1.4"/>`,
  ),
  save: icon(
    `<path d="M3 4.2A1.7 1.7 0 0 1 4.7 2.5h6.9L13.5 4.4v7.4a1.7 1.7 0 0 1-1.7 1.7H4.7A1.7 1.7 0 0 1 3 11.8z"/><path d="M5.6 2.5v3.4h4.4V2.5M5.4 13.5v-3.3h5.2v3.3"/>`,
  ),
};

const TOOLS: ReadonlyArray<{ id: ToolId; label: string; hint: string; icon: string }> = [
  {
    id: "select",
    label: "Select",
    hint: "Select and move annotations (V)",
    icon: icon(`<path d="M4.6 2.8 12 9.3l-3.6.5 1.6 3.4-1.7.8-1.6-3.5-2.1 2z"/>`),
  },
  {
    id: "crop",
    label: "Crop",
    hint: "Crop the capture (C)",
    icon: icon(`<path d="M4.6 2v9.4a1 1 0 0 0 1 1H14"/><path d="M11.4 2v9.4a1 1 0 0 1-1 1H2"/>`),
  },
  {
    id: "arrow",
    label: "Arrow",
    hint: "Draw an arrow (A)",
    icon: icon(`<path d="M3.4 12.6 12 4"/><path d="M6.2 4H12v5.8"/>`),
  },
  {
    id: "text",
    label: "Text",
    hint: "Add a text label (T)",
    icon: icon(`<path d="M3.4 4.4V2.8h9.2v1.6"/><path d="M8 2.8v10.4M6.2 13.2h3.6"/>`),
  },
  {
    id: "rect",
    label: "Box",
    hint: "Draw a rectangle (R)",
    icon: icon(`<rect x="2.6" y="3.6" width="10.8" height="8.8" rx="1.4"/>`),
  },
  {
    id: "blur",
    label: "Blur",
    hint: "Blur sensitive areas (B)",
    icon: icon(
      `<path d="M8 2.2c2.2 2.6 3.6 4.6 3.6 6.2a3.6 3.6 0 0 1-7.2 0c0-1.6 1.4-3.6 3.6-6.2z"/>`,
    ),
  },
  {
    id: "step",
    label: "Step",
    hint: "Numbered callout steps (S)",
    icon: icon(`<circle cx="8" cy="8" r="5.5"/><path d="M7.1 6.3 8.2 5.5v5M6.6 10.5h3.2"/>`),
  },
];

const COLORS: ReadonlyArray<{ name: string; hex: string }> = [
  { name: "Red", hex: "#ff3b30" },
  { name: "Orange", hex: "#ff9500" },
  { name: "Yellow", hex: "#ffcc00" },
  { name: "Green", hex: "#34c759" },
  { name: "Blue", hex: "#007aff" },
  { name: "Purple", hex: "#8b7cf6" },
  { name: "Black", hex: "#1c1c1e" },
  { name: "White", hex: "#ffffff" },
];

const STROKES: ReadonlyArray<{ id: string; width: number; label: string }> = [
  { id: "S", width: 2, label: "Thin stroke" },
  { id: "M", width: 3, label: "Medium stroke" },
  { id: "L", width: 5, label: "Thick stroke" },
];

const DEFAULT_COLOR = "#ff3b30";
const DEFAULT_STROKE = 3;
const MODE_KEY = "capicorn:capture-mode";

const errText = (e: unknown): string =>
  typeof e === "string" ? e : e instanceof Error ? e.message : String(e);

const readMode = (): CaptureMode => {
  try {
    return localStorage.getItem(MODE_KEY) === "region" ? "region" : "full";
  } catch {
    return "full";
  }
};

const writeMode = (mode: CaptureMode): void => {
  try {
    localStorage.setItem(MODE_KEY, mode);
  } catch {
    /* storage unavailable — mode just isn't remembered */
  }
};

export function mountApp(root: HTMLElement): void {
  root.innerHTML = `
    <header class="topbar">
      <div class="brand">
        <span class="brand-mark" aria-hidden="true">✦</span>
        <span class="brand-name">Capicorn</span>
        <span class="version" id="version" hidden></span>
      </div>
      <div class="topbar-right">
        <span class="topbar-hint"><kbd>PrintScreen</kbd> full · <span class="key-pair"><kbd>Shift</kbd><kbd>PrintScreen</kbd></span> region</span>
        <div class="split-btn" id="capture-split">
          <button type="button" class="btn-primary split-main" id="capture" aria-keyshortcuts="PrintScreen" title="Capture full screen (PrintScreen)">
            ${ICONS.capture}
            <span>Capture</span>
          </button>
          <span class="split-divider" aria-hidden="true"></span>
          <button type="button" class="btn-primary split-caret" id="capture-caret" aria-haspopup="menu" aria-expanded="false" aria-controls="capture-menu" aria-label="Capture options" title="Capture options">
            ${ICONS.caret}
          </button>
          <div class="capture-menu" id="capture-menu" role="menu" aria-label="Capture mode" hidden>
            <button type="button" class="menu-item" role="menuitemradio" data-mode="full" aria-checked="true">
              <span class="menu-icon">${ICONS.full}</span>
              <span class="menu-label">Full screen</span>
              <span class="menu-check">${ICONS.check}</span>
              <kbd class="menu-key">PrintScreen</kbd>
            </button>
            <button type="button" class="menu-item" role="menuitemradio" data-mode="region" aria-checked="false">
              <span class="menu-icon">${ICONS.region}</span>
              <span class="menu-label">Select region</span>
              <span class="menu-check">${ICONS.check}</span>
              <kbd class="menu-key">Shift+PrintScreen</kbd>
            </button>
          </div>
        </div>
      </div>
    </header>

    <nav class="toolbar" aria-label="Annotation tools">
      <div class="tool-group" id="tools" role="toolbar" aria-label="Tools"></div>
      <span class="toolbar-sep" aria-hidden="true"></span>
      <div class="tool-group">
        <button type="button" class="chip chip-icon" id="undo" title="Undo (Ctrl+Z)" aria-label="Undo">${ICONS.undo}</button>
        <button type="button" class="chip chip-icon" id="redo" title="Redo (Ctrl+Shift+Z)" aria-label="Redo">${ICONS.redo}</button>
      </div>
      <span class="toolbar-sep" aria-hidden="true"></span>
      <div class="tool-group toolbar-actions">
        <button type="button" class="chip" id="copy" disabled title="Copy to clipboard (Ctrl+C)">${ICONS.copy}<span>Copy</span></button>
        <button type="button" class="chip" id="save" disabled title="Save as PNG (Ctrl+S)">${ICONS.save}<span>Save</span></button>
      </div>
    </nav>

    <nav class="toolbar toolbar-ink" aria-label="Ink style">
      <div class="ink-group">
        <span class="ink-label">Ink</span>
        <div class="swatches" id="swatches" role="radiogroup" aria-label="Annotation color"></div>
        <label class="color-custom" title="Custom color">
          <input type="color" id="color-custom" value="${DEFAULT_COLOR}" aria-label="Custom annotation color" />
          <span class="color-custom-ring" aria-hidden="true"></span>
        </label>
      </div>
      <span class="toolbar-sep" aria-hidden="true"></span>
      <div class="ink-group">
        <span class="ink-label">Width</span>
        <div class="strokes" id="strokes" role="radiogroup" aria-label="Stroke width"></div>
      </div>
      <span class="ink-hint">Applies to new annotations</span>
    </nav>

    <main class="stage">
      <div class="editor-shell" id="editor"></div>
      <div class="empty-state" id="empty">
        <span class="empty-mark" aria-hidden="true">✦</span>
        <p class="empty-title">No capture yet</p>
        <p class="empty-text">Press <kbd>PrintScreen</kbd> for the full screen, <span class="key-pair"><kbd>Shift</kbd><kbd>PrintScreen</kbd></span> for a region — or pick a mode from <strong>Capture</strong>.</p>
      </div>
    </main>

    <footer class="statusbar">
      <span class="status-dot" id="dot" data-kind="info" aria-hidden="true"></span>
      <span class="status-text" id="status" role="status" aria-live="polite">Ready</span>
      <span class="status-keys"><kbd>Ctrl</kbd><kbd>Z</kbd> undo · <kbd>Ctrl</kbd><kbd>S</kbd> save</span>
    </footer>`;

  const $ = <T extends HTMLElement>(sel: string): T => root.querySelector(sel) as T;

  const editorEl = $<HTMLDivElement>("#editor");
  const stageEl = $<HTMLElement>(".stage");
  const emptyEl = $<HTMLDivElement>("#empty");
  const statusEl = $<HTMLSpanElement>("#status");
  const dotEl = $<HTMLSpanElement>("#dot");
  const versionEl = $<HTMLSpanElement>("#version");
  const toolsEl = $<HTMLDivElement>("#tools");
  const captureBtn = $<HTMLButtonElement>("#capture");
  const caretBtn = $<HTMLButtonElement>("#capture-caret");
  const menuEl = $<HTMLDivElement>("#capture-menu");
  const splitEl = $<HTMLDivElement>("#capture-split");
  const undoBtn = $<HTMLButtonElement>("#undo");
  const redoBtn = $<HTMLButtonElement>("#redo");
  const copyBtn = $<HTMLButtonElement>("#copy");
  const saveBtn = $<HTMLButtonElement>("#save");
  const swatchesEl = $<HTMLDivElement>("#swatches");
  const strokesEl = $<HTMLDivElement>("#strokes");
  const customColorEl = $<HTMLInputElement>("#color-custom");

  const editor = new Editor(editorEl) as InkEditor;
  const toolButtons = new Map<ToolId, HTMLButtonElement>();
  let hasImage = false;
  let busy = false;
  let mode: CaptureMode = readMode();

  function setStatus(message: string, kind: StatusKind = "info"): void {
    statusEl.textContent = message;
    dotEl.dataset.kind = kind;
  }

  function syncHistory(): void {
    let canUndo = false;
    let canRedo = false;
    try {
      canUndo = hasImage && editor.canUndo();
      canRedo = hasImage && editor.canRedo();
    } catch {
      canUndo = false;
      canRedo = false;
    }
    undoBtn.disabled = !canUndo;
    redoBtn.disabled = !canRedo;
  }

  function setImageState(loaded: boolean): void {
    hasImage = loaded;
    stageEl.classList.toggle("has-image", loaded);
    emptyEl.setAttribute("aria-hidden", String(loaded));
    copyBtn.disabled = !loaded;
    saveBtn.disabled = !loaded;
    syncHistory();
  }

  /* ---------------------------------------------------------------- capture */

  function setMenuOpen(open: boolean): void {
    menuEl.hidden = !open;
    splitEl.classList.toggle("is-open", open);
    caretBtn.setAttribute("aria-expanded", String(open));
    if (open) {
      const active = menuEl.querySelector<HTMLButtonElement>(".menu-item.is-active");
      (active ?? menuEl.querySelector<HTMLButtonElement>(".menu-item"))?.focus({
        preventScroll: true,
      });
    }
  }

  function setMode(next: CaptureMode, announce = false): void {
    mode = next;
    writeMode(next);
    for (const item of Array.from(menuEl.querySelectorAll<HTMLButtonElement>(".menu-item"))) {
      const active = item.dataset.mode === next;
      item.classList.toggle("is-active", active);
      item.setAttribute("aria-checked", String(active));
    }
    captureBtn.title =
      next === "full"
        ? "Capture full screen (PrintScreen)"
        : "Select a region to capture (Shift+PrintScreen)";
    captureBtn.setAttribute("aria-label", next === "full" ? "Capture full screen" : "Select region to capture");
    if (announce) {
      setStatus(
        next === "full"
          ? "Capture mode: full screen."
          : "Capture mode: select region — drag on the screen after capture.",
      );
    }
  }

  function setBusy(value: boolean): void {
    busy = value;
    captureBtn.disabled = value;
    caretBtn.disabled = value;
    captureBtn.classList.toggle("is-busy", value);
    if (value) setMenuOpen(false);
  }

  async function runCapture(requested: CaptureMode = mode): Promise<void> {
    if (busy) return;
    setBusy(true);
    setStatus(requested === "full" ? "Capturing screen…" : "Capturing screen for region…");
    try {
      const res = await invoke<CaptureResult>("capture_screenshot");
      if (requested === "full") {
        await editor.load(res.dataUrl);
        setImageState(true);
        setStatus(`Captured ${res.width} × ${res.height}`, "success");
        return;
      }

      setStatus("Drag to select a region — Enter to confirm, Esc to cancel.");
      const box = await selectRegion(res.dataUrl, stageEl);
      if (!box) {
        setStatus("Region capture cancelled");
        return;
      }
      const cropped = await cropDataUrl(res.dataUrl, box);
      await editor.load(cropped);
      setImageState(true);
      setStatus(`Captured region ${box.width} × ${box.height}`, "success");
    } catch (e) {
      setStatus(`Capture failed: ${errText(e)}`, "error");
    } finally {
      setBusy(false);
    }
  }

  /* ------------------------------------------------------------------- ink */

  function highlightColor(hex: string): void {
    const normalized = hex.toLowerCase();
    let matched = false;
    for (const btn of Array.from(swatchesEl.querySelectorAll<HTMLButtonElement>(".swatch"))) {
      const active = (btn.dataset.hex || "").toLowerCase() === normalized;
      if (active) matched = true;
      btn.classList.toggle("is-active", active);
      btn.setAttribute("aria-checked", String(active));
    }
    customColorEl.closest(".color-custom")?.classList.toggle("is-active", !matched);
    if (!matched) customColorEl.value = normalized;
  }

  function applyColor(hex: string, announce = true): void {
    try {
      editor.setColor(hex);
    } catch (e) {
      setStatus(`Color failed: ${errText(e)}`, "error");
      return;
    }
    highlightColor(hex);
    if (announce) {
      const named = COLORS.find((c) => c.hex.toLowerCase() === hex.toLowerCase());
      setStatus(`Ink color: ${named ? named.name.toLowerCase() : hex}`);
    }
  }

  function highlightStroke(width: number): void {
    for (const btn of Array.from(strokesEl.querySelectorAll<HTMLButtonElement>(".stroke"))) {
      const active = Number(btn.dataset.width) === width;
      btn.classList.toggle("is-active", active);
      btn.setAttribute("aria-checked", String(active));
    }
  }

  function applyStroke(width: number, announce = true): void {
    try {
      editor.setStrokeWidth(width);
    } catch (e) {
      setStatus(`Stroke failed: ${errText(e)}`, "error");
      return;
    }
    highlightStroke(width);
    if (announce) setStatus(`Stroke width: ${width} px`);
  }

  /* ---------------------------------------------------------------- wiring */

  toolsEl.innerHTML = TOOLS.map(
    (t) =>
      `<button type="button" class="chip tool" data-tool="${t.id}" title="${t.hint}" aria-label="${t.label}" aria-pressed="false">${t.icon}<span>${t.label}</span></button>`,
  ).join("");

  for (const btn of Array.from(toolsEl.querySelectorAll<HTMLButtonElement>(".tool"))) {
    const id = btn.dataset.tool as ToolId;
    toolButtons.set(id, btn);
    btn.addEventListener("click", () => selectTool(id));
  }

  swatchesEl.innerHTML = COLORS.map(
    (c) =>
      `<button type="button" class="swatch" role="radio" data-hex="${c.hex}" aria-checked="false" aria-label="${c.name}" title="${c.name}" style="--swatch:${c.hex}"><span class="swatch-fill"></span></button>`,
  ).join("");

  for (const btn of Array.from(swatchesEl.querySelectorAll<HTMLButtonElement>(".swatch"))) {
    btn.addEventListener("click", () => applyColor(btn.dataset.hex || DEFAULT_COLOR));
  }

  strokesEl.innerHTML = STROKES.map(
    (s) =>
      `<button type="button" class="stroke" role="radio" data-width="${s.width}" aria-checked="false" aria-label="${s.label}" title="${s.label} — ${s.width}px"><span class="stroke-dot" style="--dot:${s.width}px"></span><span class="stroke-letter">${s.id}</span></button>`,
  ).join("");

  for (const btn of Array.from(strokesEl.querySelectorAll<HTMLButtonElement>(".stroke"))) {
    btn.addEventListener("click", () => applyStroke(Number(btn.dataset.width)));
  }

  customColorEl.addEventListener("input", () => applyColor(customColorEl.value, false));
  customColorEl.addEventListener("change", () => applyColor(customColorEl.value, false));

  function selectTool(id: ToolId, announce = true): void {
    editor.setTool(id);
    for (const [key, btn] of toolButtons) {
      const active = key === id;
      btn.classList.toggle("is-active", active);
      btn.setAttribute("aria-pressed", String(active));
    }
    if (announce && !hasImage) {
      const tool = TOOLS.find((t) => t.id === id);
      setStatus(`${tool ? tool.label : id} tool selected — capture a screen to start annotating.`);
    }
  }

  async function copyPng(): Promise<void> {
    if (!hasImage) {
      setStatus("Nothing to copy yet — capture a screen first.", "error");
      return;
    }
    try {
      await invoke("copy_png", { dataUrl: editor.flatten() });
      setStatus("Copied to clipboard", "success");
    } catch (e) {
      setStatus(`Copy failed: ${errText(e)}`, "error");
    }
  }

  async function savePng(): Promise<void> {
    if (!hasImage) {
      setStatus("Nothing to save yet — capture a screen first.", "error");
      return;
    }
    try {
      const path = await invoke<string>("save_png", {
        dataUrl: editor.flatten(),
        suggestedName: "capicorn.png",
      });
      setStatus(`Saved ${path}`, "success");
    } catch (e) {
      setStatus(`Save failed: ${errText(e)}`, "error");
    }
  }

  captureBtn.addEventListener("click", () => void runCapture());
  caretBtn.addEventListener("click", () => setMenuOpen(menuEl.hidden));

  for (const item of Array.from(menuEl.querySelectorAll<HTMLButtonElement>(".menu-item"))) {
    item.addEventListener("click", () => {
      const next: CaptureMode = item.dataset.mode === "region" ? "region" : "full";
      setMenuOpen(false);
      setMode(next);
      void runCapture(next);
    });
  }

  menuEl.addEventListener("keydown", (ev) => {
    const items = Array.from(menuEl.querySelectorAll<HTMLButtonElement>(".menu-item"));
    const idx = items.indexOf(document.activeElement as HTMLButtonElement);
    if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      const step = ev.key === "ArrowDown" ? 1 : -1;
      const next = (idx + step + items.length) % items.length;
      items[next]?.focus();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      setMenuOpen(false);
      caretBtn.focus();
    }
  });

  const onDocPointerDown = (ev: PointerEvent): void => {
    if (menuEl.hidden) return;
    const target = ev.target as Node | null;
    if (target && splitEl.contains(target)) return;
    setMenuOpen(false);
  };
  document.addEventListener("pointerdown", onDocPointerDown);

  editorEl.addEventListener("capicorn:change", (ev) => {
    const detail = (ev as CustomEvent<ChangeDetail>).detail;
    const canUndo = typeof detail?.canUndo === "boolean" ? detail.canUndo : editor.canUndo();
    const canRedo = typeof detail?.canRedo === "boolean" ? detail.canRedo : editor.canRedo();
    undoBtn.disabled = !hasImage || !canUndo;
    redoBtn.disabled = !hasImage || !canRedo;
  });

  undoBtn.addEventListener("click", () => {
    editor.undo();
    syncHistory();
  });
  redoBtn.addEventListener("click", () => {
    editor.redo();
    syncHistory();
  });
  copyBtn.addEventListener("click", () => void copyPng());
  saveBtn.addEventListener("click", () => void savePng());

  const onKeyDown = (ev: KeyboardEvent): void => {
    if (ev.code === "PrintScreen" || ev.key === "PrintScreen") {
      ev.preventDefault();
      void runCapture(ev.shiftKey ? "region" : "full");
      return;
    }
    const target = ev.target as HTMLElement | null;
    const typing =
      !!target &&
      (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable);
    if (typing || !(ev.ctrlKey || ev.metaKey) || ev.altKey) return;

    const key = ev.key.toLowerCase();
    if (key === "z") {
      ev.preventDefault();
      if (ev.shiftKey) editor.redo();
      else editor.undo();
      syncHistory();
    } else if (key === "y") {
      ev.preventDefault();
      editor.redo();
      syncHistory();
    } else if (key === "c") {
      ev.preventDefault();
      void copyPng();
    } else if (key === "s") {
      ev.preventDefault();
      void savePng();
    }
  };
  window.addEventListener("keydown", onKeyDown);

  const onUnload = (): void => {
    window.removeEventListener("keydown", onKeyDown);
    document.removeEventListener("pointerdown", onDocPointerDown);
    editor.destroy();
  };
  window.addEventListener("beforeunload", onUnload, { once: true });

  selectTool("select", false);
  setImageState(false);
  setMode(mode);

  try {
    applyColor(editor.getColor() ?? DEFAULT_COLOR, false);
    applyStroke(editor.getStrokeWidth() ?? DEFAULT_STROKE, false);
  } catch {
    highlightColor(DEFAULT_COLOR);
    highlightStroke(DEFAULT_STROKE);
  }

  setStatus("Ready — press PrintScreen for the full screen, Shift+PrintScreen for a region.");

  void (async () => {
    try {
      const version = await invoke<string>("get_version");
      if (version) {
        versionEl.textContent = version.startsWith("v") ? version : `v${version}`;
        versionEl.hidden = false;
      }
    } catch {
      versionEl.hidden = true;
    }
  })();
}