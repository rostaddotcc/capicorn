// Capicorn — theme tokens, export settings and the settings modal.
//
// The app shell owns the live state (save folder, version, theme, export
// defaults); this module owns applying the theme to <html>, the persistence
// helpers for the export/general preferences, and a self-contained, accessible
// settings dialog. No Tauri imports: the shell passes its callbacks in, so the
// panel stays testable in a plain browser.

export type Theme = "light" | "dark";
export type ExportFormat = "png" | "jpeg" | "webp";

export const THEME_KEY = "capicorn:theme";

/** Stored theme, defaulting to light (rostad's default, spec Wave 4). */
export function readTheme(): Theme {
  try {
    return localStorage.getItem(THEME_KEY) === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

/** Apply a theme to the document root; persists unless told otherwise. */
export function applyTheme(theme: Theme, persist = true): void {
  const root = document.documentElement;
  root.dataset.theme = theme;
  root.style.colorScheme = theme;
  if (!persist) return;
  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch {
    /* storage unavailable — the theme just isn't remembered */
  }
}

/* ---------------------------------------------------------------- settings */

export const FORMAT_KEY = "capicorn:format";
export const QUALITY_KEY = "capicorn:quality";
export const TIMER_KEY = "capicorn:timer";
export const FILENAME_KEY = "capicorn:filename";
export const TRAY_KEY = "capicorn:minimize-tray";

export const DEFAULT_FORMAT: ExportFormat = "png";
export const DEFAULT_QUALITY = 90;
export const DEFAULT_TIMER = 0;
export const DEFAULT_PATTERN = "Capicorn-{date}-{time}";
/** Delays offered in Settings → Capture. */
export const TIMER_CHOICES: ReadonlyArray<number> = [0, 3, 5, 10];

export interface ExportSettings {
  format: ExportFormat;
  quality: number;
  /** Delayed-capture countdown in seconds; 0 captures immediately. */
  timer: number;
  /** Filename pattern with {date}/{time}/{datetime} tokens. */
  filename: string;
  minimizeToTray: boolean;
}

const store = (key: string): string | null => {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
};

const clampInt = (raw: string | null, fallback: number, min: number, max: number): number => {
  if (raw === null || raw.trim() === "") return fallback;
  const value = Number(raw);
  if (!Number.isFinite(value)) return fallback;
  return Math.min(max, Math.max(min, Math.round(value)));
};

export const isExportFormat = (value: string): value is ExportFormat =>
  value === "png" || value === "jpeg" || value === "webp";

/** Stored export preferences, each field falling back to its default. */
export function readExportSettings(): ExportSettings {
  const rawFormat = store(FORMAT_KEY) ?? "";
  const rawTimer = clampInt(store(TIMER_KEY), DEFAULT_TIMER, 0, 60);
  const rawPattern = store(FILENAME_KEY);
  return {
    format: isExportFormat(rawFormat) ? rawFormat : DEFAULT_FORMAT,
    quality: clampInt(store(QUALITY_KEY), DEFAULT_QUALITY, 1, 100),
    // Only the offered delays (plus a stored custom value) are honoured.
    timer: TIMER_CHOICES.includes(rawTimer) ? rawTimer : DEFAULT_TIMER,
    filename:
      rawPattern !== null && rawPattern.trim() !== "" ? rawPattern : DEFAULT_PATTERN,
    minimizeToTray: store(TRAY_KEY) === "true",
  };
}

/** Persist the whole export-preference block in one pass. */
export function saveExportSettings(settings: ExportSettings): void {
  try {
    localStorage.setItem(FORMAT_KEY, settings.format);
    localStorage.setItem(QUALITY_KEY, String(settings.quality));
    localStorage.setItem(TIMER_KEY, String(settings.timer));
    localStorage.setItem(FILENAME_KEY, settings.filename);
    localStorage.setItem(TRAY_KEY, String(settings.minimizeToTray));
  } catch {
    /* storage unavailable — the preferences just aren't remembered */
  }
}

/** File extension used on disk for a format ('jpeg' writes .jpg). */
export function formatExtension(format: ExportFormat): string {
  return format === "jpeg" ? "jpg" : format;
}

/** Human label used in status messages and the settings select. */
export function formatLabel(format: ExportFormat): string {
  return format === "jpeg" ? "JPEG" : format === "webp" ? "WebP" : "PNG";
}

const pad = (n: number): string => String(n).padStart(2, "0");

/**
 * Expand the filename pattern: {date} → 2026-11-14, {time} → 19-45-12,
 * {datetime} → 2026-11-14-19-45-12. Unknown tokens are left alone so a typo
 * never silently disappears, and path separators are stripped so the file
 * always lands in the chosen folder.
 */
export function expandPattern(pattern: string, date = new Date()): string {
  const day = `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  const clock = `${pad(date.getHours())}-${pad(date.getMinutes())}-${pad(date.getSeconds())}`;
  const expanded = (pattern.trim() === "" ? DEFAULT_PATTERN : pattern)
    .replace(/\{date\}/gi, day)
    .replace(/\{time\}/gi, clock)
    .replace(/\{datetime\}/gi, `${day}-${clock}`)
    // eslint-disable-next-line no-control-regex
    .replace(/[\\/:*?"<>|\u0000-\u001f]/g, "-")
    .replace(/\s+/g, " ")
    .trim();
  return expanded === "" ? `Capicorn-${day}-${clock}` : expanded;
}

/** Full filename for a capture, extension included. */
export function buildFilename(
  pattern: string,
  format: ExportFormat,
  date = new Date(),
  suffix = "",
): string {
  return `${expandPattern(pattern, date)}${suffix}.${formatExtension(format)}`;
}

export interface SettingsOptions {
  /** Current save folder, or null while it is still unresolved. */
  getSaveDir(): string | null;
  /** Open the native folder picker; resolves to the new folder (or null). */
  changeSaveDir(): Promise<string | null>;
  /** Version string for the footer, when the shell has one. */
  getVersion(): string | null;
  /** Called when the user picks a theme inside the panel. */
  setTheme(theme: Theme): void;
  /** Live export/general preferences. */
  getSettings(): ExportSettings;
  /**
   * Persist + apply a change. `silent` is for continuous controls (slider
   * drags, filename typing) that must not spam the status bar.
   */
  updateSettings(patch: Partial<ExportSettings>, opts?: { silent?: boolean }): void;
}

const icon = (body: string, size = 16): string =>
  `<svg viewBox="0 0 16 16" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;

const ICONS = {
  sun: icon(
    `<circle cx="8" cy="8" r="2.9"/><path d="M8 1.7v1.6M8 12.7v1.6M1.7 8h1.6M12.7 8h1.6M3.5 3.5l1.1 1.1M11.4 11.4l1.1 1.1M12.5 3.5l-1.1 1.1M4.6 11.4l-1.1 1.1"/>`,
    14,
  ),
  moon: icon(`<path d="M13.1 9.7A5.5 5.5 0 0 1 6.3 2.9a5.6 5.6 0 1 0 6.8 6.8z"/>`, 14),
  close: icon(`<path d="M4.2 4.2l7.6 7.6M11.8 4.2l-7.6 7.6"/>`, 14),
  caret: icon(`<path d="M4.6 6.4 8 9.8l3.4-3.4"/>`, 12),
  folder: icon(
    `<path d="M2.3 5.4A1.5 1.5 0 0 1 3.8 3.9h2.1l1.3 1.6h5a1.5 1.5 0 0 1 1.5 1.5v4a1.5 1.5 0 0 1-1.5 1.5H3.8a1.5 1.5 0 0 1-1.5-1.5z"/>`,
    14,
  ),
};

const FOCUSABLE =
  'button:not(:disabled), [href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

export class SettingsPanel {
  private readonly opts: SettingsOptions;
  private root: HTMLDivElement | null = null;
  private card!: HTMLDivElement;
  private dirPath!: HTMLElement;
  private versionEl!: HTMLElement;
  private segments: HTMLButtonElement[] = [];
  private changeBtn!: HTMLButtonElement;
  private timerPills: HTMLButtonElement[] = [];
  private formatSelect!: HTMLSelectElement;
  private qualityField!: HTMLElement;
  private qualityInput!: HTMLInputElement;
  private qualityOut!: HTMLOutputElement;
  private nameInput!: HTMLInputElement;
  private namePreview!: HTMLElement;
  private traySwitch!: HTMLButtonElement;
  private lastFocus: HTMLElement | null = null;
  private picking = false;

  constructor(opts: SettingsOptions) {
    this.opts = opts;
  }

  isOpen(): boolean {
    return !!this.root && !this.root.hidden;
  }

  open(): void {
    const root = this.ensure();
    if (!root.hidden) return;
    this.lastFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    this.refresh();
    root.hidden = false;
    // Force a style pass so the entrance transition runs from its start state.
    void root.offsetWidth;
    root.classList.add("is-open");
    this.card.focus({ preventScroll: true });
  }

  close(): void {
    const root = this.root;
    if (!root || root.hidden) return;
    root.classList.remove("is-open");
    root.hidden = true;
    this.lastFocus?.focus({ preventScroll: true });
    this.lastFocus = null;
  }

  /** Re-render the whole panel from the shell's live state (idempotent). */
  refresh(theme: Theme = readTheme()): void {
    if (!this.root) return;
    const dir = this.opts.getSaveDir();
    this.dirPath.textContent = dir ?? "Not set yet";
    this.dirPath.title = dir ?? "";
    this.dirPath.classList.toggle("is-empty", !dir);

    const version = this.opts.getVersion();
    this.versionEl.textContent = version ? `Capicorn ${version}` : "Capicorn";

    for (const segment of this.segments) {
      const active = segment.dataset.themeChoice === theme;
      segment.classList.toggle("is-active", active);
      segment.setAttribute("aria-checked", String(active));
      segment.tabIndex = active ? 0 : -1;
    }

    const settings = this.opts.getSettings();

    for (const pill of this.timerPills) {
      const active = Number(pill.dataset.timer) === settings.timer;
      pill.classList.toggle("is-active", active);
      pill.setAttribute("aria-checked", String(active));
      pill.tabIndex = active ? 0 : -1;
    }

    this.formatSelect.value = settings.format;
    this.paintQuality(settings.format, settings.quality);

    // Never fight the caret while the pattern is being typed.
    if (document.activeElement !== this.nameInput) {
      this.nameInput.value = settings.filename;
    }
    this.paintPreview();

    this.traySwitch.classList.toggle("is-on", settings.minimizeToTray);
    this.traySwitch.setAttribute("aria-checked", String(settings.minimizeToTray));
    this.traySwitch.title = settings.minimizeToTray ? "Minimize to tray is on" : "Minimize to tray is off";
  }

  /* ------------------------------------------------------------- internals */

  private paintQuality(format: ExportFormat, quality: number): void {
    const lossy = format !== "png";
    this.qualityField.hidden = !lossy;
    this.qualityInput.value = String(quality);
    this.qualityOut.textContent = String(quality);
    this.qualityInput.style.setProperty("--fill", `${((quality - 1) / 99) * 100}%`);
  }

  private paintPreview(): void {
    const settings = this.opts.getSettings();
    const sample = expandPattern(this.nameInput.value || settings.filename);
    this.namePreview.textContent = `${sample}.${formatExtension(settings.format)}`;
    this.namePreview.classList.toggle("is-fallback", this.nameInput.value.trim() === "");
  }

  private ensure(): HTMLDivElement {
    if (this.root) return this.root;

    const root = document.createElement("div");
    root.className = "modal-backdrop";
    root.hidden = true;
    root.innerHTML = `
      <div class="modal" role="dialog" aria-modal="true" aria-labelledby="settings-title" aria-describedby="settings-desc" tabindex="-1">
        <header class="modal-head">
          <div class="modal-heading">
            <h2 class="modal-title" id="settings-title">Settings</h2>
            <p class="modal-desc" id="settings-desc">Capture behaviour, where files go, and how they are written.</p>
          </div>
          <button type="button" class="icon-btn modal-x" data-close aria-label="Close settings" title="Close (Esc)">${ICONS.close}</button>
        </header>
        <div class="modal-body">
          <section class="setting" data-section="general">
            <h3 class="setting-title">General</h3>
            <div class="setting-row">
              <div class="setting-row-text">
                <span class="setting-label" id="tray-label">Minimize to tray</span>
                <span class="setting-sub">Closing the window keeps Capicorn running in the tray.</span>
              </div>
              <button type="button" class="switch" id="settings-tray" role="switch" aria-checked="false" aria-labelledby="tray-label"><span class="switch-knob" aria-hidden="true"></span></button>
            </div>
          </section>

          <section class="setting" data-section="capture">
            <h3 class="setting-title">Capture</h3>
            <p class="setting-help">Wait before grabbing the screen, so you can set the shot up first.</p>
            <div class="pills" id="settings-timer" role="radiogroup" aria-label="Capture delay">
              ${TIMER_CHOICES.map(
                (seconds) =>
                  `<button type="button" role="radio" data-timer="${seconds}" aria-checked="${seconds === 0}">${seconds === 0 ? "Off" : `${seconds} s`}</button>`,
              ).join("")}
            </div>
          </section>

          <section class="setting" data-section="save">
            <h3 class="setting-title">Save location</h3>
            <p class="setting-help">Every capture is written here. Edited copies join it in the same folder.</p>
            <div class="dir-row">
              <span class="dir-icon" aria-hidden="true">${ICONS.folder}</span>
              <code class="dir-path" id="settings-dir">Not set yet</code>
              <button type="button" class="btn-secondary" id="settings-change">Change…</button>
            </div>
          </section>

          <section class="setting" data-section="export">
            <h3 class="setting-title">Export</h3>
            <p class="setting-help">PNG stays lossless. JPEG and WebP are far smaller — quality controls how much detail they keep.</p>
            <div class="field">
              <label class="field-label" for="settings-format">Format</label>
              <div class="select-wrap">
                <select id="settings-format" class="field-select">
                  <option value="png">PNG · lossless, sharpest text</option>
                  <option value="jpeg">JPEG · small, no transparency</option>
                  <option value="webp">WebP · small and modern</option>
                </select>
                <span class="select-arrow" aria-hidden="true">${ICONS.caret}</span>
              </div>
            </div>
            <div class="field" id="settings-quality-field" hidden>
              <label class="field-label" for="settings-quality">Quality <output class="field-value" id="settings-quality-out" for="settings-quality">90</output></label>
              <input type="range" class="slider" id="settings-quality" min="1" max="100" step="1" value="90" aria-label="Export quality" />
            </div>
            <div class="field">
              <label class="field-label" for="settings-filename">Filename pattern</label>
              <input type="text" class="field-input" id="settings-filename" value="${DEFAULT_PATTERN}" spellcheck="false" autocomplete="off" aria-describedby="settings-tokens settings-preview" />
              <p class="token-hint" id="settings-tokens"><code>{date}</code> 2026-11-14 · <code>{time}</code> 19-45-12 · <code>{datetime}</code> both</p>
              <p class="preview-line" id="settings-preview">Preview: <code class="preview-name">${DEFAULT_PATTERN}.png</code></p>
            </div>
          </section>

          <section class="setting" data-section="theme">
            <h3 class="setting-title">Theme</h3>
            <p class="setting-help">Light for daylight, dark for late sessions.</p>
            <div class="segmented" role="radiogroup" aria-label="Theme">
              <button type="button" role="radio" data-theme-choice="light" aria-checked="true">${ICONS.sun}<span>Light</span></button>
              <button type="button" role="radio" data-theme-choice="dark" aria-checked="false" tabindex="-1">${ICONS.moon}<span>Dark</span></button>
            </div>
          </section>
        </div>
        <footer class="modal-foot">
          <span class="modal-version" id="settings-version"></span>
          <button type="button" class="btn-secondary" data-close>Close</button>
        </footer>
      </div>`;

    this.root = root;
    this.card = root.querySelector<HTMLDivElement>(".modal")!;
    this.dirPath = root.querySelector<HTMLElement>("#settings-dir")!;
    this.versionEl = root.querySelector<HTMLElement>("#settings-version")!;
    this.changeBtn = root.querySelector<HTMLButtonElement>("#settings-change")!;
    this.segments = Array.from(root.querySelectorAll<HTMLButtonElement>("[data-theme-choice]"));
    this.timerPills = Array.from(root.querySelectorAll<HTMLButtonElement>("[data-timer]"));
    this.formatSelect = root.querySelector<HTMLSelectElement>("#settings-format")!;
    this.qualityField = root.querySelector<HTMLElement>("#settings-quality-field")!;
    this.qualityInput = root.querySelector<HTMLInputElement>("#settings-quality")!;
    this.qualityOut = root.querySelector<HTMLOutputElement>("#settings-quality-out")!;
    this.nameInput = root.querySelector<HTMLInputElement>("#settings-filename")!;
    this.namePreview = root.querySelector<HTMLElement>("#settings-preview .preview-name")!;
    this.traySwitch = root.querySelector<HTMLButtonElement>("#settings-tray")!;

    for (const closer of Array.from(root.querySelectorAll<HTMLElement>("[data-close]"))) {
      closer.addEventListener("click", () => this.close());
    }

    for (const segment of this.segments) {
      segment.addEventListener("click", () => {
        const choice: Theme = segment.dataset.themeChoice === "dark" ? "dark" : "light";
        this.opts.setTheme(choice);
        this.refresh(choice);
      });
    }

    this.changeBtn.addEventListener("click", () => void this.pickFolder());

    this.traySwitch.addEventListener("click", () => {
      const next = !this.opts.getSettings().minimizeToTray;
      this.opts.updateSettings({ minimizeToTray: next });
      this.refresh();
    });

    for (const pill of this.timerPills) {
      pill.addEventListener("click", () => {
        this.opts.updateSettings({ timer: Number(pill.dataset.timer) });
        this.refresh();
      });
    }

    this.formatSelect.addEventListener("change", () => {
      const value = this.formatSelect.value;
      const format: ExportFormat = isExportFormat(value) ? value : DEFAULT_FORMAT;
      this.opts.updateSettings({ format });
      this.refresh();
      this.paintPreview();
    });

    this.qualityInput.addEventListener("input", () => {
      const quality = clampInt(this.qualityInput.value, DEFAULT_QUALITY, 1, 100);
      this.opts.updateSettings({ quality }, { silent: true });
      this.paintQuality(this.opts.getSettings().format, quality);
    });
    this.qualityInput.addEventListener("change", () => {
      this.opts.updateSettings({ quality: clampInt(this.qualityInput.value, DEFAULT_QUALITY, 1, 100) });
      this.paintQuality(this.opts.getSettings().format, this.opts.getSettings().quality);
    });

    this.nameInput.addEventListener("input", () => {
      this.opts.updateSettings({ filename: this.nameInput.value }, { silent: true });
      this.paintPreview();
    });
    this.nameInput.addEventListener("blur", () => {
      if (this.nameInput.value.trim() === "") {
        this.opts.updateSettings({ filename: DEFAULT_PATTERN }, { silent: true });
      }
      this.opts.updateSettings({ filename: this.opts.getSettings().filename });
      this.refresh();
    });

    // Click on the scrim (not the card) closes.
    root.addEventListener("pointerdown", (ev) => {
      if (ev.target === root) this.close();
    });

    root.addEventListener("keydown", this.onKeyDown);
    // Focus can sit on <body> after clicking non-focusable chrome, so Esc is
    // also caught at the document — but only while this dialog is topmost.
    document.addEventListener("keydown", this.onDocKeyDown, true);
    document.body.append(root);
    return root;
  }

  /** True when no other visible modal sits above this one. */
  private isTopmost(): boolean {
    if (!this.root || this.root.hidden) return false;
    const mine = Number(getComputedStyle(this.root).zIndex) || 0;
    return Array.from(document.querySelectorAll<HTMLElement>(".modal-backdrop:not([hidden])")).every(
      (el) => el === this.root || (Number(getComputedStyle(el).zIndex) || 0) <= mine,
    );
  }

  private readonly onDocKeyDown = (ev: KeyboardEvent): void => {
    if (ev.key !== "Escape" || !this.isTopmost()) return;
    ev.preventDefault();
    ev.stopPropagation();
    this.close();
  };

  private async pickFolder(): Promise<void> {
    if (this.picking) return;
    this.picking = true;
    const label = this.changeBtn.textContent;
    this.changeBtn.disabled = true;
    this.changeBtn.textContent = "Choosing…";
    try {
      await this.opts.changeSaveDir();
    } finally {
      this.picking = false;
      this.changeBtn.disabled = false;
      this.changeBtn.textContent = label ?? "Change…";
      this.refresh();
    }
  }

  private readonly onKeyDown = (ev: KeyboardEvent): void => {
    if (!this.root) return;
    if (ev.key === "Escape") {
      ev.preventDefault();
      ev.stopPropagation();
      this.close();
      return;
    }
    if (ev.key === "ArrowRight" || ev.key === "ArrowLeft") {
      const group = this.segments.includes(document.activeElement as HTMLButtonElement)
        ? this.segments
        : this.timerPills.includes(document.activeElement as HTMLButtonElement)
          ? this.timerPills
          : null;
      if (!group) return;
      ev.preventDefault();
      const index = group.findIndex((s) => s === document.activeElement);
      const step = ev.key === "ArrowRight" ? 1 : -1;
      const next = group[(index + step + group.length) % group.length];
      next?.focus();
      next?.click();
      return;
    }
    if (ev.key !== "Tab") return;

    const items = Array.from(this.root.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
      (el) => el.offsetParent !== null,
    );
    if (!items.length) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement as HTMLElement | null;
    if (ev.shiftKey && (active === first || !this.root.contains(active))) {
      ev.preventDefault();
      last.focus();
    } else if (!ev.shiftKey && (active === last || !this.root.contains(active))) {
      ev.preventDefault();
      first.focus();
    }
  };
}