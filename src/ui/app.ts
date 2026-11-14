// Capicorn app shell — top bar, annotation toolbar, editor stage, status line.
// Talks to the locked Editor API and the locked Tauri command set only.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Editor, type ToolId } from "../editor/editor";
import { cropDataUrl, selectRegion } from "./region";
import {
  DEFAULT_PATTERN,
  SettingsPanel,
  applyTheme,
  buildFilename,
  formatLabel,
  readExportSettings,
  readTheme,
  saveExportSettings,
  type ExportFormat,
  type ExportSettings,
  type Theme,
} from "./settings";

type CaptureResult = { dataUrl: string; width: number; height: number };
type StatusKind = "info" | "success" | "error";
type CaptureMode = "full" | "region";

/** Additive Editor API (Wave 2/3, contract-locked). */
type InkApi = {
  setColor(hex: string): void;
  getColor(): string;
  setStrokeWidth(width: number): void;
  getStrokeWidth(): number;
};
type InkEditor = Editor & InkApi;

/**
 * Wave 4 added the bucket tool and Wave 5 adds spotlight / highlighter /
 * callout to `ToolId`. Widen locally so the shell typechecks against either
 * revision of the editor module while the tracks land.
 */
type ShellToolId = ToolId | "bucket" | "spotlight" | "highlighter" | "callout";

/** Wave 5 blur tuning. Optional so the shell builds against either editor revision. */
type BlurMode = "gaussian" | "pixelate" | "blackout";
type BlurApi = {
  setBlurMode?(mode: BlurMode): void;
  getBlurMode?(): BlurMode;
  setBlurRadius?(radius: number): void;
  getBlurRadius?(): number;
};

type ChangeDetail = { tool?: ToolId; canUndo?: boolean; canRedo?: boolean };

const icon = (body: string, size = 16, weight = 1.4): string =>
  `<svg viewBox="0 0 16 16" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="${weight}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;

const ICONS = {
  capture: icon(
    `<rect x="2.4" y="3" width="11.2" height="7.6" rx="1.5"/><path d="M6 13.4h4M8 10.6v2.8"/>`,
  ),
  caret: icon(`<path d="M4.6 6.4 8 9.8l3.4-3.4"/>`, 12),
  full: icon(`<rect x="2.2" y="2.8" width="11.6" height="8.4" rx="1.4"/><path d="M5.4 13.4h5.2"/>`),
  region: icon(
    `<path d="M2.6 6V3.6A1 1 0 0 1 3.6 2.6H6M10 2.6h2.4a1 1 0 0 1 1 1V6M13.4 10v2.4a1 1 0 0 1-1 1H10M6 13.4H3.6a1 1 0 0 1-1-1V10"/>`,
  ),
  check: icon(`<path d="M3 8.6 6.4 12l6.6-7.6"/>`, 14, 2.1),
  undo: icon(`<path d="M6.2 4.2 3.4 7l2.8 2.8"/><path d="M3.6 7h5.2a3.6 3.6 0 0 1 0 7.2H6.4"/>`),
  redo: icon(`<path d="M9.8 4.2 12.6 7l-2.8 2.8"/><path d="M12.4 7H7.2a3.6 3.6 0 0 0 0 7.2h2.4"/>`),
  copy: icon(
    `<rect x="5.5" y="5.5" width="7.5" height="7.5" rx="1.6"/><path d="M10.5 5.5V4.1A1.6 1.6 0 0 0 8.9 2.5H4.1A1.6 1.6 0 0 0 2.5 4.1v4.8a1.6 1.6 0 0 0 1.6 1.6h1.4"/>`,
  ),
  save: icon(
    `<path d="M3 4.2A1.7 1.7 0 0 1 4.7 2.5h6.9L13.5 4.4v7.4a1.7 1.7 0 0 1-1.7 1.7H4.7A1.7 1.7 0 0 1 3 11.8z"/><path d="M5.6 2.5v3.4h4.4V2.5M5.4 13.5v-3.3h5.2v3.3"/>`,
  ),
  moon: icon(`<path d="M13.1 9.7A5.5 5.5 0 0 1 6.3 2.9a5.6 5.6 0 1 0 6.8 6.8z"/>`),
  sun: icon(
    `<circle cx="8" cy="8" r="2.9"/><path d="M8 1.7v1.6M8 12.7v1.6M1.7 8h1.6M12.7 8h1.6M3.5 3.5l1.1 1.1M11.4 11.4l1.1 1.1M12.5 3.5l-1.1 1.1M4.6 11.4l-1.1 1.1"/>`,
  ),
  settings: icon(
    `<path d="M2.6 5.2h10.8M2.6 10.8h10.8"/><circle cx="6" cy="5.2" r="1.8"/><circle cx="10.2" cy="10.8" r="1.8"/>`,
  ),
};

const TOOLS: ReadonlyArray<{ id: ShellToolId; label: string; hint: string; icon: string }> = [
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
    id: "bucket",
    label: "Bucket",
    hint: "Fill an area with ink (G)",
    icon: icon(
      `<path d="M6.6 2.2 2.9 5.9a1.4 1.4 0 0 0 0 2l3.2 3.2a1.4 1.4 0 0 0 2 0l3.7-3.7z"/><path d="M4.6 4.2 8.6 8.2"/><path d="M13.2 9.7c0 .9-.6 1.6-1.4 1.6s-1.4-.7-1.4-1.6c0-.7 1.4-2.2 1.4-2.2s1.4 1.5 1.4 2.2z"/>`,
    ),
  },
  {
    id: "spotlight",
    label: "Spotlight",
    hint: "Dim everything outside a region (L)",
    icon: icon(
      `<circle cx="8" cy="8" r="3.1"/><path d="M2.4 2.4 4.5 4.5M13.6 2.4 11.5 4.5M2.4 13.6 4.5 11.5M13.6 13.6 11.5 11.5"/>`,
    ),
  },
  {
    id: "highlighter",
    label: "Highlight",
    hint: "Swipe a translucent highlighter (H)",
    icon: icon(
      `<path d="M3.6 12.4 8.9 7.1l2 2-5.3 5.3-2.6.7z"/><path d="M8.9 7.1 10.7 5.3l2 2-1.8 1.8z"/><path d="M2.8 14.4h4.4"/>`,
    ),
  },
  {
    id: "callout",
    label: "Callout",
    hint: "Speech-bubble note (K)",
    icon: icon(
      `<path d="M2.6 3.4h10.8v6.6H8.4l-3 2.8V10H2.6z"/><path d="M5.2 6.2h5.6M5.2 8h3.4"/>`,
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
const DEFAULT_FONT = "Inter";
const DEFAULT_SIZE = 32;
const MODE_KEY = "capicorn:capture-mode";
const INK_KEY = "capicorn:ink-color";
const STROKE_KEY = "capicorn:stroke-width";
const DIR_KEY = "capicorn:save-dir";
const FONT_KEY = "capicorn:font-family";
const SIZE_KEY = "capicorn:font-size";
const BOLD_KEY = "capicorn:text-bold";
const ITALIC_KEY = "capicorn:text-italic";
const BLUR_MODE_KEY = "capicorn:blur-mode";
const BLUR_RADIUS_KEY = "capicorn:blur-radius";
const DEFAULT_BLUR_MODE: BlurMode = "gaussian";
const DEFAULT_BLUR_RADIUS = 12;
const MIN_BLUR_RADIUS = 2;
const MAX_BLUR_RADIUS = 60;

/** Text formatting slice of the Editor (landed by track E alongside this shell). */
type TextApi = {
  setFontFamily?(family: string): void;
  getFontFamily?(): string;
  setFontSize?(size: number): void;
  getFontSize?(): number;
  setTextBold?(bold: boolean): void;
  isTextBold?(): boolean;
  setTextItalic?(italic: boolean): void;
  isTextItalic?(): boolean;
};

/** Families bundled under src/assets/fonts (OFL). `css` is a safe font-family token. */
const FONTS: ReadonlyArray<{ family: string; css: string; stack: string }> = [
  { family: "Inter", css: "Inter", stack: "'Inter', system-ui, sans-serif" },
  { family: "Source Serif 4", css: "'Source Serif 4'", stack: "'Source Serif 4', Georgia, serif" },
  {
    family: "JetBrains Mono",
    css: "'JetBrains Mono'",
    stack: "'JetBrains Mono', ui-monospace, monospace",
  },
  { family: "Caveat", css: "Caveat", stack: "'Caveat', cursive" },
];

const errText = (e: unknown): string =>
  typeof e === "string" ? e : e instanceof Error ? e.message : String(e);

const readStore = (key: string): string | null => {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
};

const writeStore = (key: string, value: string): void => {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* storage unavailable — the preference just isn't remembered */
  }
};

const readMode = (): CaptureMode => (readStore(MODE_KEY) === "region" ? "region" : "full");

const readColor = (): string => {
  const stored = readStore(INK_KEY) ?? "";
  return /^#[0-9a-f]{6}$/i.test(stored) ? stored.toLowerCase() : DEFAULT_COLOR;
};

const readStroke = (): number => {
  const raw = readStore(STROKE_KEY);
  if (raw === null || raw.trim() === "") return DEFAULT_STROKE;
  const stored = Number(raw);
  if (!Number.isFinite(stored) || stored <= 0) return DEFAULT_STROKE;
  return Math.min(12, Math.max(1, Math.round(stored)));
};

const readFont = (): string => {
  const stored = readStore(FONT_KEY) ?? "";
  return FONTS.some((f) => f.family === stored) ? stored : DEFAULT_FONT;
};

const readSize = (): number => {
  const raw = readStore(SIZE_KEY);
  if (raw === null || raw.trim() === "") return DEFAULT_SIZE;
  const stored = Number(raw);
  if (!Number.isFinite(stored) || stored <= 0) return DEFAULT_SIZE;
  return Math.min(200, Math.max(8, Math.round(stored)));
};

const readBool = (key: string): boolean => readStore(key) === "true";

const readBlurMode = (): BlurMode => {
  const stored = readStore(BLUR_MODE_KEY) ?? "";
  return stored === "pixelate" || stored === "blackout" ? stored : DEFAULT_BLUR_MODE;
};

const readBlurRadius = (): number => {
  const raw = readStore(BLUR_RADIUS_KEY);
  if (raw === null || raw.trim() === "") return DEFAULT_BLUR_RADIUS;
  const stored = Number(raw);
  if (!Number.isFinite(stored) || stored <= 0) return DEFAULT_BLUR_RADIUS;
  return Math.min(MAX_BLUR_RADIUS, Math.max(MIN_BLUR_RADIUS, Math.round(stored)));
};

export function mountApp(root: HTMLElement): void {
  const currentTheme = readTheme();
  applyTheme(currentTheme);

  root.innerHTML = `
    <header class="topbar">
      <div class="brand">
        <span class="brand-mark" aria-hidden="true">✦</span>
        <span class="brand-name">Capicorn</span>
        <span class="version" id="version" hidden></span>
      </div>
      <div class="topbar-right">
        <span class="topbar-hint"><kbd>PrintScreen</kbd> full · <span class="key-pair"><kbd>Shift</kbd><kbd>PrintScreen</kbd></span> region</span>
        <div class="topbar-tools">
          <button type="button" class="icon-btn" id="theme-toggle" title="Switch to dark theme" aria-label="Switch to dark theme" aria-pressed="false">${currentTheme === "dark" ? ICONS.sun : ICONS.moon}</button>
          <button type="button" class="icon-btn" id="settings-btn" title="Settings (Ctrl+,)" aria-label="Settings">${ICONS.settings}</button>
        </div>
        <div class="split-btn" id="capture-split">
          <button type="button" class="btn-primary split-main" id="capture" aria-keyshortcuts="PrintScreen" title="Capture full screen (PrintScreen)">
            ${ICONS.capture}
            <span>Capture</span>
            <span class="split-mode" id="capture-mode-label">Full</span>
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
        <button type="button" class="chip" id="save" disabled title="Save an edited copy (Ctrl+S)">${ICONS.save}<span>Save</span></button>
      </div>
    </nav>

    <nav class="toolbar toolbar-ink" aria-label="Ink style">
      <div class="ink-group">
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

    <nav class="textbar" id="textbar" aria-label="Text formatting" hidden>
      <span class="textbar-label">Text</span>
      <label class="font-picker" title="Font family">
        <span class="sr-only">Font family</span>
        <select id="font-family" class="font-select" aria-label="Font family">
          <option value="Inter" style="font-family:'Inter',system-ui,sans-serif">Inter</option>
          <option value="Source Serif 4" style="font-family:'Source Serif 4',Georgia,serif">Source Serif 4</option>
          <option value="JetBrains Mono" style="font-family:'JetBrains Mono',ui-monospace,monospace">JetBrains Mono</option>
          <option value="Caveat" style="font-family:'Caveat',cursive">Caveat</option>
        </select>
        <span class="font-picker-arrow" aria-hidden="true">${ICONS.caret}</span>
      </label>
      <span class="toolbar-sep" aria-hidden="true"></span>
      <div class="size-stepper" role="group" aria-label="Font size">
        <button type="button" class="step-btn" id="size-down" aria-label="Smaller text" title="Smaller">−</button>
        <input type="number" id="font-size" class="size-input" min="8" max="200" step="1" value="${DEFAULT_SIZE}" aria-label="Font size in pixels" />
        <button type="button" class="step-btn" id="size-up" aria-label="Larger text" title="Larger">+</button>
      </div>
      <span class="toolbar-sep" aria-hidden="true"></span>
      <div class="text-toggle-group" role="group" aria-label="Text style">
        <button type="button" class="text-toggle" id="text-bold" aria-pressed="false" title="Bold" aria-label="Bold"><span class="text-toggle-glyph" style="font-weight:700">B</span></button>
        <button type="button" class="text-toggle" id="text-italic" aria-pressed="false" title="Italic" aria-label="Italic"><span class="text-toggle-glyph" style="font-style:italic;font-family:'Source Serif 4',Georgia,serif">I</span></button>
      </div>
      <span class="textbar-hint" id="textbar-hint">Click anywhere to add text</span>
    </nav>

    <nav class="blurbar" id="blurbar" aria-label="Blur options" hidden>
      <span class="textbar-label">Blur</span>
      <div class="segmented segmented-sm" id="blur-modes" role="radiogroup" aria-label="Blur mode">
        <button type="button" role="radio" data-blur-mode="gaussian" aria-checked="false">Gaussian</button>
        <button type="button" role="radio" data-blur-mode="pixelate" aria-checked="false">Pixelate</button>
        <button type="button" role="radio" data-blur-mode="blackout" aria-checked="false">Blackout</button>
      </div>
      <span class="toolbar-sep" aria-hidden="true"></span>
      <div class="radius-field" id="blur-radius-field">
        <label class="radius-label" for="blur-radius">Intensity</label>
        <input type="range" class="slider slider-sm" id="blur-radius" min="2" max="60" step="1" value="${DEFAULT_BLUR_RADIUS}" aria-label="Blur intensity" />
        <output class="radius-value" id="blur-radius-out" for="blur-radius">${DEFAULT_BLUR_RADIUS}</output>
      </div>
      <span class="blurbar-hint" id="blur-hint">Drag over anything private.</span>
    </nav>

    <main class="stage">
      <div class="editor-shell" id="editor"></div>
      <div class="empty-state" id="empty">
        <span class="empty-mark" aria-hidden="true">✦</span>
        <p class="empty-title">No capture yet</p>
        <p class="empty-text">Press <kbd>PrintScreen</kbd> for the full screen, <span class="key-pair"><kbd>Shift</kbd><kbd>PrintScreen</kbd></span> for a region — or pick a mode from <strong>Capture</strong>.</p>
        <p class="empty-note">Every capture is saved to your save folder automatically.</p>
      </div>
    </main>

    <footer class="statusbar">
      <span class="status-dot" id="dot" data-kind="info" aria-hidden="true"></span>
      <span class="status-text" id="status" role="status" aria-live="polite">Ready</span>
      <span class="status-keys"><kbd>Ctrl</kbd><kbd>Z</kbd> undo · <kbd>Ctrl</kbd><kbd>S</kbd> save copy</span>
    </footer>

    <div class="countdown" id="countdown" hidden role="dialog" aria-modal="true" aria-labelledby="countdown-num">
      <div class="countdown-card">
        <p class="countdown-kicker" id="countdown-kicker">Capture in</p>
        <span class="countdown-num" id="countdown-num" aria-live="assertive">3</span>
        <p class="countdown-hint" id="countdown-hint">Full screen · press Esc to cancel</p>
        <button type="button" class="btn-secondary countdown-cancel" id="countdown-cancel">Cancel</button>
      </div>
    </div>

    <div class="modal-backdrop close-guard" id="close-guard" hidden>
      <div class="modal modal-sm" role="alertdialog" aria-modal="true" aria-labelledby="guard-title" aria-describedby="guard-desc" tabindex="-1">
        <header class="modal-head">
          <div class="modal-heading">
            <h2 class="modal-title" id="guard-title">Save changes before closing?</h2>
            <p class="modal-desc" id="guard-desc">Your annotations are not on disk yet.</p>
          </div>
        </header>
        <div class="modal-body guard-body">
          <p class="guard-swatch"><span class="guard-dot" aria-hidden="true"></span><span id="guard-file">Capicorn capture</span></p>
        </div>
        <footer class="modal-foot guard-foot">
          <button type="button" class="btn-secondary" id="guard-cancel">Cancel</button>
          <div class="guard-actions">
            <button type="button" class="btn-ghost-danger" id="guard-discard">Discard</button>
            <button type="button" class="btn-primary" id="guard-save">Save &amp; close</button>
          </div>
        </footer>
      </div>
    </div>`;

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
  const modeLabelEl = $<HTMLSpanElement>("#capture-mode-label");
  const menuEl = $<HTMLDivElement>("#capture-menu");
  const splitEl = $<HTMLDivElement>("#capture-split");
  const themeBtn = $<HTMLButtonElement>("#theme-toggle");
  const settingsBtn = $<HTMLButtonElement>("#settings-btn");
  const undoBtn = $<HTMLButtonElement>("#undo");
  const redoBtn = $<HTMLButtonElement>("#redo");
  const copyBtn = $<HTMLButtonElement>("#copy");
  const saveBtn = $<HTMLButtonElement>("#save");
  const swatchesEl = $<HTMLDivElement>("#swatches");
  const strokesEl = $<HTMLDivElement>("#strokes");
  const customColorEl = $<HTMLInputElement>("#color-custom");
  const textBar = $<HTMLElement>("#textbar");
  const fontSelect = $<HTMLSelectElement>("#font-family");
  const sizeInput = $<HTMLInputElement>("#font-size");
  const sizeDownBtn = $<HTMLButtonElement>("#size-down");
  const sizeUpBtn = $<HTMLButtonElement>("#size-up");
  const boldBtn = $<HTMLButtonElement>("#text-bold");
  const italicBtn = $<HTMLButtonElement>("#text-italic");
  const blurBar = $<HTMLElement>("#blurbar");
  const blurModesEl = $<HTMLDivElement>("#blur-modes");
  const blurRadiusInput = $<HTMLInputElement>("#blur-radius");
  const blurRadiusOut = $<HTMLOutputElement>("#blur-radius-out");
  const blurHint = $<HTMLSpanElement>("#blur-hint");
  const radiusField = $<HTMLDivElement>("#blur-radius-field");
  const countdownEl = $<HTMLDivElement>("#countdown");
  const countdownNum = $<HTMLSpanElement>("#countdown-num");
  const countdownHint = $<HTMLParagraphElement>("#countdown-hint");
  const countdownCancelBtn = $<HTMLButtonElement>("#countdown-cancel");
  const guardEl = $<HTMLDivElement>("#close-guard");
  const guardFileEl = $<HTMLSpanElement>("#guard-file");
  const guardSaveBtn = $<HTMLButtonElement>("#guard-save");
  const guardDiscardBtn = $<HTMLButtonElement>("#guard-discard");
  const guardCancelBtn = $<HTMLButtonElement>("#guard-cancel");

  const editor = new Editor(editorEl) as InkEditor;
  const toolButtons = new Map<ShellToolId, HTMLButtonElement>();
  let hasImage = false;
  let busy = false;
  let mode: CaptureMode = readMode();
  let theme: Theme = currentTheme;
  let saveDir: string | null = readStore(DIR_KEY);
  let panel: SettingsPanel | null = null;
  let settings: ExportSettings = readExportSettings();
  let blurMode: BlurMode = readBlurMode();
  let blurRadius = readBlurRadius();
  /** Live countdown, so a second capture (or Esc) can cancel the first. */
  let countdownCancel: (() => void) | null = null;
  let guardBusy = false;

  function setStatus(message: string, kind: StatusKind = "info"): void {
    statusEl.textContent = message;
    dotEl.dataset.kind = kind;
  }

  /* --------------------------------------------------------------- theme */

  function syncThemeButton(): void {
    const dark = theme === "dark";
    themeBtn.innerHTML = dark ? ICONS.sun : ICONS.moon;
    const label = dark ? "Switch to light theme" : "Switch to dark theme";
    themeBtn.title = label;
    themeBtn.setAttribute("aria-label", label);
    themeBtn.setAttribute("aria-pressed", String(dark));
  }

  function setTheme(next: Theme, persist = true): void {
    theme = next;
    applyTheme(next, persist);
    syncThemeButton();
    panel?.refresh(next);
  }

  /* -------------------------------------------------------------- settings */

  /** Persist + apply an export/general preference change from the panel. */
  function updateSettings(patch: Partial<ExportSettings>, opts: { silent?: boolean } = {}): void {
    if (patch.minimizeToTray !== undefined) {
      const enabled = patch.minimizeToTray;
      const previous = settings.minimizeToTray;
      settings = { ...settings, minimizeToTray: enabled };
      saveExportSettings(settings);
      void invoke("set_minimize_to_tray", { enabled }).catch(() => {
        settings = { ...settings, minimizeToTray: previous };
        saveExportSettings(settings);
        panel?.refresh();
        setStatus("Minimize to tray is not available in this build.", "error");
      });
      if (!opts.silent) setStatus(enabled ? "Closing minimizes Capicorn to the tray" : "Closing quits Capicorn");
      panel?.refresh();
      return;
    }

    settings = { ...settings, ...patch };
    saveExportSettings(settings);
    if (!opts.silent) {
      if (patch.timer !== undefined) {
        setStatus(
          settings.timer > 0
            ? `Capture delay: ${settings.timer} s — press Capture to start the countdown`
            : "Capture delay: off — captures immediately",
        );
      } else if (patch.format !== undefined) {
        setStatus(
          `Export format: ${formatLabel(settings.format)}${
            settings.format === "png" ? "" : ` · quality ${settings.quality}`
          }`,
        );
      } else if (patch.quality !== undefined) {
        setStatus(`Export quality: ${settings.quality}`);
      } else if (patch.filename !== undefined) {
        setStatus(`Filename pattern: ${expandName(settings.filename)}`);
      }
    }
    panel?.refresh();
  }

  /** Filename the next save would produce (guard text + status). */
  const expandName = (pattern: string): string => buildFilename(pattern, settings.format, new Date());

  /* --------------------------------------------------------------- saving */

  /** Resolve the save folder, seeding it from the backend on first run. */
  async function ensureSaveDir(): Promise<string | null> {
    if (saveDir) return saveDir;
    try {
      const dir = await invoke<string>("default_save_dir");
      const trimmed = typeof dir === "string" ? dir.trim() : "";
      if (!trimmed) return null;
      saveDir = trimmed;
      writeStore(DIR_KEY, trimmed);
      panel?.refresh();
      return trimmed;
    } catch (e) {
      setStatus(`Could not determine the save folder: ${errText(e)}`, "error");
      return null;
    }
  }

  async function pickSaveDir(): Promise<string | null> {
    try {
      const dir = await invoke<string>("pick_save_dir");
      const trimmed = typeof dir === "string" ? dir.trim() : "";
      if (!trimmed) return saveDir;
      saveDir = trimmed;
      writeStore(DIR_KEY, trimmed);
      setStatus(`Save folder: ${trimmed}`);
      panel?.refresh();
      return trimmed;
    } catch (e) {
      const message = errText(e);
      if (/cancel/i.test(message)) {
        setStatus("Folder change cancelled");
      } else {
        setStatus(`Could not change folder: ${message}`, "error");
      }
      return saveDir;
    }
  }

  /**
   * Write a data URL through `save_image` using the chosen format, quality and
   * filename pattern. Throws on failure so each caller can word its own status.
   */
  async function writeImage(
    dataUrl: string,
    edited: boolean,
  ): Promise<{ path: string; filename: string } | null> {
    const dir = await ensureSaveDir();
    if (!dir) return null;
    const filename = buildFilename(
      settings.filename,
      settings.format,
      new Date(),
      edited ? "-edited" : "",
    );
    const path = await invoke<string>("save_image", {
      dataUrl,
      dir,
      filename,
      format: settings.format,
      quality: Math.round(settings.quality),
    });
    return { path: typeof path === "string" && path ? path : `${dir}/${filename}`, filename };
  }

  /** Auto-save a freshly loaded capture. Never throws — capture must survive. */
  async function autoSave(dataUrl: string): Promise<void> {
    try {
      const saved = await writeImage(dataUrl, false);
      if (!saved) return;
      setStatus(`Saved ${saved.filename}`, "success");
    } catch (e) {
      setStatus(`Capture kept in the editor — auto-save failed: ${errText(e)}`, "error");
    }
  }

  /** Ctrl+S / Save: only writes when the capture actually changed. */
  async function saveEdited(): Promise<boolean> {
    if (!hasImage) {
      setStatus("Nothing to save yet — capture a screen first.", "error");
      return false;
    }
    if (!isDirty()) {
      setStatus("No changes to save");
      return false;
    }
    try {
      const saved = await writeImage(editor.flatten(), true);
      if (!saved) return false;
      setStatus(`Saved copy ${saved.filename}`, "success");
      syncDirty();
      return true;
    } catch (e) {
      setStatus(`Save failed: ${errText(e)}`, "error");
      return false;
    }
  }

  /** Wave 4 adds Editor#isDirty; fall back to history depth if it has not landed. */
  function isDirty(): boolean {
    const probe = editor as unknown as { isDirty?: () => boolean };
    return typeof probe.isDirty === "function" ? probe.isDirty() : editor.canUndo();
  }

  /** Tell the Rust side whether closing needs a guard. Best-effort. */
  function syncDirty(): void {
    void invoke("set_dirty", { dirty: hasImage && isDirty() }).catch(() => {
      /* older backend without set_dirty — the guard then simply never fires */
    });
  }

  /* ------------------------------------------------------------ countdown */

  /**
   * Show a cancellable countdown before the grab. Resolves true when it runs
   * out and false when the user cancels (Esc, the button, or a new capture).
   */
  function runCountdown(seconds: number, requested: CaptureMode): Promise<boolean> {
    countdownCancel?.();
    return new Promise<boolean>((resolve) => {
      let remaining = Math.max(1, Math.round(seconds));
      let settled = false;
      const timers: number[] = [];

      const settle = (capture: boolean): void => {
        if (settled) return;
        settled = true;
        for (const id of timers) window.clearTimeout(id);
        timers.length = 0;
        countdownCancelBtn.removeEventListener("click", cancel);
        document.removeEventListener("keydown", cancel, true);
        countdownEl.classList.remove("is-open");
        countdownEl.hidden = true;
        document.body.classList.remove("is-counting");
        if (countdownCancel === cancel) countdownCancel = null;
        resolve(capture);
      };

      const cancel = (): void => settle(false);

      const tick = (): void => {
        if (remaining <= 0) {
          settle(true);
          return;
        }
        countdownNum.textContent = String(remaining);
        countdownNum.classList.remove("is-tick");
        void countdownNum.offsetWidth;
        countdownNum.classList.add("is-tick");
        remaining -= 1;
        timers.push(window.setTimeout(tick, 1000));
      };

      countdownHint.textContent =
        requested === "full" ? "Full screen · Esc to cancel" : "Region · Esc to cancel";
      countdownEl.hidden = false;
      document.body.classList.add("is-counting");
      void countdownEl.offsetWidth;
      countdownEl.classList.add("is-open");
      countdownCancel = cancel;
      countdownCancelBtn.addEventListener("click", cancel);
      document.addEventListener("keydown", cancel, true);
      countdownCancelBtn.focus({ preventScroll: true });
      tick();
    });
  }

  /* ------------------------------------------------------------ close guard */

  function guardKeyDown(ev: KeyboardEvent): void {
    if (guardEl.hidden) return;
    if (ev.key === "Escape") {
      ev.preventDefault();
      ev.stopPropagation();
      dismissGuard("Close cancelled — your capture is still here.");
      return;
    }
    if (ev.key !== "Tab") return;
    const items = [guardCancelBtn, guardDiscardBtn, guardSaveBtn].filter((b) => !b.disabled);
    const active = document.activeElement as HTMLElement | null;
    const index = items.indexOf(active as HTMLButtonElement);
    ev.preventDefault();
    const step = ev.shiftKey ? -1 : 1;
    const next = items[(index + step + items.length) % items.length];
    next?.focus();
  }

  function dismissGuard(message?: string): void {
    if (guardEl.hidden) return;
    guardEl.classList.remove("is-open");
    guardEl.hidden = true;
    document.removeEventListener("keydown", guardKeyDown, true);
    guardBusy = false;
    guardSaveBtn.disabled = false;
    guardDiscardBtn.disabled = false;
    if (message) setStatus(message);
  }

  function showGuard(): void {
    if (!guardEl.hidden) return;
    countdownCancel?.();
    guardFileEl.textContent = expandName(settings.filename);
    guardEl.hidden = false;
    void guardEl.offsetWidth;
    guardEl.classList.add("is-open");
    guardSaveBtn.disabled = !(hasImage && isDirty());
    guardDiscardBtn.disabled = false;
    guardCancelBtn.disabled = false;
    document.addEventListener("keydown", guardKeyDown, true);
    (guardSaveBtn.disabled ? guardDiscardBtn : guardSaveBtn).focus({ preventScroll: true });
  }

  /** Ask Rust to close for real; resolves false when the invoke failed. */
  async function forceClose(): Promise<boolean> {
    try {
      await invoke("force_close");
      return true;
    } catch (e) {
      setStatus(`Could not close the window: ${errText(e)}`, "error");
      return false;
    }
  }

  async function onGuardSave(): Promise<void> {
    if (guardBusy) return;
    guardBusy = true;
    guardSaveBtn.disabled = true;
    guardDiscardBtn.disabled = true;
    guardCancelBtn.disabled = true;
    if (hasImage && isDirty()) {
      try {
        const saved = await writeImage(editor.flatten(), true);
        if (!saved) {
          guardBusy = false;
          guardSaveBtn.disabled = false;
          guardDiscardBtn.disabled = false;
          guardCancelBtn.disabled = false;
          return;
        }
        setStatus(`Saved copy ${saved.filename}`, "success");
      } catch (e) {
        setStatus(`Save failed: ${errText(e)}`, "error");
        guardBusy = false;
        guardSaveBtn.disabled = false;
        guardDiscardBtn.disabled = false;
        guardCancelBtn.disabled = false;
        return;
      }
    }
    const closed = await forceClose();
    if (closed) {
      guardBusy = false;
      dismissGuard();
    } else {
      guardBusy = false;
      guardSaveBtn.disabled = !(hasImage && isDirty());
      guardDiscardBtn.disabled = false;
      guardCancelBtn.disabled = false;
    }
  }

  async function onGuardDiscard(): Promise<void> {
    if (guardBusy) return;
    guardBusy = true;
    guardSaveBtn.disabled = true;
    guardDiscardBtn.disabled = true;
    guardCancelBtn.disabled = true;
    const closed = await forceClose();
    if (closed) {
      guardBusy = false;
      dismissGuard();
    } else {
      guardBusy = false;
      guardSaveBtn.disabled = !(hasImage && isDirty());
      guardDiscardBtn.disabled = false;
      guardCancelBtn.disabled = false;
    }
  }

  /* ------------------------------------------------------------- history */

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
    syncDirty();
  }

  /* ---------------------------------------------------------------- capture */

  function setMenuOpen(open: boolean): void {
    if (open === !menuEl.hidden) return;
    menuEl.hidden = !open;
    menuEl.classList.toggle("is-open", open);
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
    writeStore(MODE_KEY, next);
    for (const item of Array.from(menuEl.querySelectorAll<HTMLButtonElement>(".menu-item"))) {
      const active = item.dataset.mode === next;
      item.classList.toggle("is-active", active);
      item.setAttribute("aria-checked", String(active));
    }
    modeLabelEl.textContent = next === "full" ? "Full" : "Region";
    captureBtn.title =
      next === "full"
        ? "Capture full screen (PrintScreen)"
        : "Select a region to capture (Shift+PrintScreen)";
    captureBtn.setAttribute(
      "aria-label",
      next === "full" ? "Capture full screen" : "Select region to capture",
    );
    if (announce) {
      setStatus(
        next === "full"
          ? "Capture mode: full screen."
          : "Capture mode: select region — press Capture, then drag on the screen.",
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

  /**
   * Let the webview settle into the new viewport size. Two animation frames are
   * enough when frames are flowing; the timeout races them so a throttled or
   * occluded window can never stall the capture flow.
   */
  function settle(): Promise<void> {
    const frames = new Promise<void>((resolve) => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    });
    const timer = new Promise<void>((resolve) => {
      window.setTimeout(resolve, 120);
    });
    return Promise.race([frames, timer]);
  }

  /**
   * Snagit-style region capture: go fullscreen so the window IS the desktop,
   * freeze the screen, drag a box over it, restore the window, then crop.
   * The window is ALWAYS restored, on success, cancel and error alike.
   */
  async function runRegionCapture(): Promise<void> {
    let full = false;
    try {
      await invoke("set_fullscreen", { full: true });
      full = true;
      await settle();

      const shot = await invoke<CaptureResult>("capture_screenshot");
      setStatus("Drag to select a region — Enter to confirm, Esc to cancel.");
      const box = await selectRegion(shot.dataUrl);

      await invoke("set_fullscreen", { full: false });
      full = false;

      if (!box) {
        setStatus("Region capture cancelled");
        return;
      }
      const cropped = await cropDataUrl(shot.dataUrl, box);
      await editor.load(cropped);
      setImageState(true);
      setStatus(`Captured region ${box.width} × ${box.height}`, "success");
      await autoSave(cropped);
    } finally {
      if (full) {
        try {
          await invoke("set_fullscreen", { full: false });
        } catch {
          /* restore failed — the window is left as-is rather than stuck half-way */
        }
      }
    }
  }

  async function runCapture(requested: CaptureMode = mode): Promise<void> {
    if (busy || !guardEl.hidden) return;
    setBusy(true);
    try {
      if (settings.timer > 0) {
        setStatus(`Capturing in ${settings.timer} s — Esc to cancel`);
        const proceed = await runCountdown(settings.timer, requested);
        if (!proceed) {
          setStatus("Capture cancelled");
          return;
        }
        // Let the overlay finish fading out before the screen is grabbed.
        await settle();
      }
      setStatus(requested === "full" ? "Capturing screen…" : "Preparing region capture…");
      if (requested === "full") {
        const res = await invoke<CaptureResult>("capture_screenshot");
        await editor.load(res.dataUrl);
        setImageState(true);
        setStatus(`Captured ${res.width} × ${res.height}`, "success");
        syncDirty();
        await autoSave(res.dataUrl);
        return;
      }
      await runRegionCapture();
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
    customColorEl.value = normalized;
  }

  function applyColor(hex: string, announce = true): void {
    try {
      editor.setColor(hex);
    } catch (e) {
      setStatus(`Color failed: ${errText(e)}`, "error");
      return;
    }
    highlightColor(hex);
    writeStore(INK_KEY, hex);
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
    writeStore(STROKE_KEY, String(width));
    if (announce) setStatus(`Stroke width: ${width} px`);
  }

  /* ------------------------------------------------------------------ text */

  const textApi = editor as unknown as TextApi;
  let fontFamily = readFont();
  let fontSize = readSize();
  let textBold = readBool(BOLD_KEY);
  let textItalic = readBool(ITALIC_KEY);

  const fontEntry = (family: string): (typeof FONTS)[number] =>
    FONTS.find((f) => f.family === family) ?? FONTS[0];

  function paintTextToggles(): void {
    boldBtn.classList.toggle("is-active", textBold);
    boldBtn.setAttribute("aria-pressed", String(textBold));
    boldBtn.title = textBold ? "Bold (on)" : "Bold";
    italicBtn.classList.toggle("is-active", textItalic);
    italicBtn.setAttribute("aria-pressed", String(textItalic));
    italicBtn.title = textItalic ? "Italic (on)" : "Italic";
  }

  function applyFont(family: string, announce = true): void {
    const entry = fontEntry(family);
    fontFamily = entry.family;
    textApi.setFontFamily?.(entry.css);
    fontSelect.value = entry.family;
    fontSelect.style.fontFamily = entry.stack;
    writeStore(FONT_KEY, entry.family);
    if (announce) setStatus(`Font: ${entry.family}`);
  }

  function applySize(size: number, announce = true): void {
    const next = Math.min(200, Math.max(8, Math.round(size) || DEFAULT_SIZE));
    fontSize = next;
    textApi.setFontSize?.(next);
    sizeInput.value = String(next);
    sizeDownBtn.disabled = next <= 8;
    sizeUpBtn.disabled = next >= 200;
    writeStore(SIZE_KEY, String(next));
    if (announce) setStatus(`Text size: ${next} px`);
  }

  function applyBold(on: boolean, announce = true): void {
    textBold = on;
    textApi.setTextBold?.(on);
    paintTextToggles();
    writeStore(BOLD_KEY, String(on));
    if (announce) setStatus(on ? "Bold on" : "Bold off");
  }

  function applyItalic(on: boolean, announce = true): void {
    textItalic = on;
    textApi.setTextItalic?.(on);
    paintTextToggles();
    writeStore(ITALIC_KEY, String(on));
    if (announce) setStatus(on ? "Italic on" : "Italic off");
  }

  /** Adopt the editor's own values once its setters/getters are present. */
  function syncTextFromEditor(): void {
    const family = textApi.getFontFamily?.();
    if (family) {
      const hit = FONTS.find((f) => family.toLowerCase().includes(f.family.toLowerCase()));
      if (hit && hit.family !== fontFamily) {
        fontFamily = hit.family;
        fontSelect.value = hit.family;
        fontSelect.style.fontFamily = hit.stack;
        writeStore(FONT_KEY, hit.family);
      }
    }
    const size = textApi.getFontSize?.();
    if (typeof size === "number" && Number.isFinite(size)) {
      const clamped = Math.min(200, Math.max(8, Math.round(size)));
      fontSize = clamped;
      sizeInput.value = String(clamped);
      sizeDownBtn.disabled = clamped <= 8;
      sizeUpBtn.disabled = clamped >= 200;
    }
    const bold = textApi.isTextBold?.();
    if (typeof bold === "boolean") textBold = bold;
    const italic = textApi.isTextItalic?.();
    if (typeof italic === "boolean") textItalic = italic;
    paintTextToggles();
  }

  /* ----------------------------------------------------------------- blur */

  const blurApi = editor as unknown as BlurApi;

  function paintBlur(): void {
    for (const btn of Array.from(
      blurModesEl.querySelectorAll<HTMLButtonElement>("[data-blur-mode]"),
    )) {
      const active = btn.dataset.blurMode === blurMode;
      btn.classList.toggle("is-active", active);
      btn.setAttribute("aria-checked", String(active));
      btn.tabIndex = active ? 0 : -1;
    }
    blurRadiusInput.value = String(blurRadius);
    blurRadiusOut.textContent = String(blurRadius);
    blurRadiusInput.style.setProperty(
      "--fill",
      `${((blurRadius - MIN_BLUR_RADIUS) / (MAX_BLUR_RADIUS - MIN_BLUR_RADIUS)) * 100}%`,
    );
    const usesIntensity = blurMode !== "blackout";
    radiusField.classList.toggle("is-muted", !usesIntensity);
    blurRadiusInput.disabled = !usesIntensity;
    blurHint.textContent =
      blurMode === "gaussian"
        ? "Gaussian softens the area — intensity sets how far it smears."
        : blurMode === "pixelate"
          ? "Pixelate blocks the area — intensity sets the block size."
          : "Blackout paints the area solid; intensity is unused.";
  }

  function applyBlurMode(next: BlurMode, announce = true): void {
    blurMode = next;
    try {
      blurApi.setBlurMode?.(next);
    } catch (e) {
      setStatus(`Blur mode failed: ${errText(e)}`, "error");
      return;
    }
    writeStore(BLUR_MODE_KEY, next);
    paintBlur();
    if (announce) setStatus(`Blur mode: ${next}`);
  }

  function applyBlurRadius(next: number, announce = true): void {
    const clamped = Math.min(
      MAX_BLUR_RADIUS,
      Math.max(MIN_BLUR_RADIUS, Math.round(next) || DEFAULT_BLUR_RADIUS),
    );
    blurRadius = clamped;
    try {
      blurApi.setBlurRadius?.(clamped);
    } catch (e) {
      setStatus(`Blur intensity failed: ${errText(e)}`, "error");
      return;
    }
    writeStore(BLUR_RADIUS_KEY, String(clamped));
    paintBlur();
    if (announce) setStatus(`Blur intensity: ${clamped}`);
  }

  /** Adopt whatever the editor reports once its blur setters are present. */
  function syncBlurFromEditor(): void {
    const mode = blurApi.getBlurMode?.();
    if (mode === "gaussian" || mode === "pixelate" || mode === "blackout") {
      blurMode = mode;
    }
    const radius = blurApi.getBlurRadius?.();
    if (typeof radius === "number" && Number.isFinite(radius)) {
      blurRadius = Math.min(MAX_BLUR_RADIUS, Math.max(MIN_BLUR_RADIUS, Math.round(radius)));
    }
    paintBlur();
  }

  /* ---------------------------------------------------------------- wiring */

  toolsEl.innerHTML = TOOLS.map(
    (t) =>
      `<button type="button" class="chip tool" data-tool="${t.id}" title="${t.hint}" aria-label="${t.label}" aria-pressed="false">${t.icon}<span>${t.label}</span></button>`,
  ).join("");

  for (const btn of Array.from(toolsEl.querySelectorAll<HTMLButtonElement>(".tool"))) {
    const id = btn.dataset.tool as ShellToolId;
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

  fontSelect.addEventListener("change", () => applyFont(fontSelect.value));
  sizeDownBtn.addEventListener("click", () => applySize(fontSize - 1));
  sizeUpBtn.addEventListener("click", () => applySize(fontSize + 1));
  sizeInput.addEventListener("change", () => applySize(Number(sizeInput.value)));
  sizeInput.addEventListener("keydown", (ev) => {
    if (ev.key !== "Enter") return;
    ev.preventDefault();
    applySize(Number(sizeInput.value));
    sizeInput.blur();
  });
  boldBtn.addEventListener("click", () => applyBold(!textBold));
  italicBtn.addEventListener("click", () => applyItalic(!textItalic));

  for (const btn of Array.from(
    blurModesEl.querySelectorAll<HTMLButtonElement>("[data-blur-mode]"),
  )) {
    btn.addEventListener("click", () => {
      const value = btn.dataset.blurMode;
      applyBlurMode(value === "pixelate" || value === "blackout" ? value : "gaussian");
    });
  }
  blurRadiusInput.addEventListener("input", () => applyBlurRadius(Number(blurRadiusInput.value), false));
  blurRadiusInput.addEventListener("change", () => applyBlurRadius(Number(blurRadiusInput.value)));
  blurModesEl.addEventListener("keydown", (ev) => {
    if (ev.key !== "ArrowRight" && ev.key !== "ArrowLeft") return;
    const order: BlurMode[] = ["gaussian", "pixelate", "blackout"];
    const index = order.indexOf(blurMode);
    ev.preventDefault();
    const step = ev.key === "ArrowRight" ? 1 : -1;
    const next = order[(index + step + order.length) % order.length];
    applyBlurMode(next);
    blurModesEl
      .querySelector<HTMLButtonElement>(`[data-blur-mode="${next}"]`)
      ?.focus({ preventScroll: true });
  });

  function selectTool(id: ShellToolId, announce = true): void {
    try {
      editor.setTool(id as ToolId);
    } catch (e) {
      setStatus(`${id} is not available in this build: ${errText(e)}`, "error");
      return;
    }
    textBar.hidden = id !== "text";
    blurBar.hidden = id !== "blur";
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

  panel = new SettingsPanel({
    getSaveDir: () => saveDir,
    changeSaveDir: () => pickSaveDir(),
    getVersion: () => (versionEl.hidden ? null : versionEl.textContent),
    setTheme: (next) => setTheme(next),
    getSettings: () => settings,
    updateSettings: (patch, opts) => updateSettings(patch, opts),
  });

  guardSaveBtn.addEventListener("click", () => void onGuardSave());
  guardDiscardBtn.addEventListener("click", () => void onGuardDiscard());
  guardCancelBtn.addEventListener("click", () =>
    dismissGuard("Close cancelled — your capture is still here."),
  );
  guardEl.addEventListener("pointerdown", (ev) => {
    if (ev.target === guardEl) dismissGuard("Close cancelled — your capture is still here.");
  });

  themeBtn.addEventListener("click", () => setTheme(theme === "dark" ? "light" : "dark"));
  settingsBtn.addEventListener("click", () => panel?.open());

  captureBtn.addEventListener("click", () => void runCapture());
  caretBtn.addEventListener("click", () => setMenuOpen(menuEl.hidden));

  // Opening from the caret with ArrowDown is a standard menu affordance.
  caretBtn.addEventListener("keydown", (ev) => {
    if (ev.key !== "ArrowDown" || !menuEl.hidden) return;
    ev.preventDefault();
    setMenuOpen(true);
  });

  for (const item of Array.from(menuEl.querySelectorAll<HTMLButtonElement>(".menu-item"))) {
    item.addEventListener("click", () => {
      const next: CaptureMode = item.dataset.mode === "region" ? "region" : "full";
      setMode(next, true);
      setMenuOpen(false);
      caretBtn.focus({ preventScroll: true });
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
    } else if (ev.key === "Home" || ev.key === "End") {
      ev.preventDefault();
      items[ev.key === "Home" ? 0 : items.length - 1]?.focus();
    } else if (ev.key === "Escape") {
      ev.preventDefault();
      setMenuOpen(false);
      caretBtn.focus({ preventScroll: true });
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
    syncDirty();
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
  saveBtn.addEventListener("click", () => void saveEdited());

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
    if (key === ",") {
      ev.preventDefault();
      panel?.open();
      return;
    }
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
      void saveEdited();
    }
  };
  window.addEventListener("keydown", onKeyDown);

  /* ------------------------------------------------------------- tauri glue */

  // Rust asks us to run a capture (tray menu item, future global hotkey).
  void listen("tray-capture", () => {
    void runCapture();
  }).catch(() => {
    /* plain browser or an older backend without the tray */
  });

  // Rust intercepts the close while the canvas is dirty and asks us to guard it.
  void listen("close-requested", () => {
    syncDirty();
    showGuard();
  }).catch(() => {
    /* no close guard outside the Tauri window */
  });

  // Keep the Rust-side tray preference mirrored with our stored value.
  void invoke("set_minimize_to_tray", { enabled: settings.minimizeToTray }).catch(() => {
    /* command missing on older builds — the toggle reports that itself */
  });

  const onUnload = (): void => {
    window.removeEventListener("keydown", onKeyDown);
    document.removeEventListener("pointerdown", onDocPointerDown);
    panel?.close();
    editor.destroy();
  };
  window.addEventListener("beforeunload", onUnload, { once: true });

  /* ------------------------------------------------------------------ init */

  selectTool("select", false);
  setImageState(false);
  setMode(mode, false);
  setTheme(theme, false);

  const savedInk = readColor();
  const savedStroke = readStroke();
  try {
    applyColor(savedInk, false);
    applyStroke(savedStroke, false);
  } catch {
    highlightColor(savedInk);
    highlightStroke(savedStroke);
    customColorEl.value = savedInk;
  }

  applyFont(fontFamily, false);
  applySize(fontSize, false);
  applyBold(textBold, false);
  applyItalic(textItalic, false);
  syncTextFromEditor();

  applyBlurMode(blurMode, false);
  applyBlurRadius(blurRadius, false);
  syncBlurFromEditor();

  setStatus("Ready — press PrintScreen for the full screen, Shift+PrintScreen for a region.");

  void (async () => {
    try {
      const version = await invoke<string>("get_version");
      if (version) {
        versionEl.textContent = version.startsWith("v") ? version : `v${version}`;
        versionEl.hidden = false;
        panel?.refresh();
      }
    } catch {
      versionEl.hidden = true;
    }
    await ensureSaveDir();
  })();
}