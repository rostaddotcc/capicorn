// Capicorn — theme tokens and the settings modal.
//
// The app shell owns the state (save folder, version, theme choice); this module
// owns applying the theme to <html> and rendering a self-contained, accessible
// settings dialog. No Tauri imports: the shell passes the folder picker in, so
// the panel stays testable in a plain browser.

export type Theme = "light" | "dark";

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

const icon = (body: string, size = 16): string =>
  `<svg viewBox="0 0 16 16" width="${size}" height="${size}" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${body}</svg>`;

const ICONS = {
  sun: icon(
    `<circle cx="8" cy="8" r="2.9"/><path d="M8 1.7v1.6M8 12.7v1.6M1.7 8h1.6M12.7 8h1.6M3.5 3.5l1.1 1.1M11.4 11.4l1.1 1.1M12.5 3.5l-1.1 1.1M4.6 11.4l-1.1 1.1"/>`,
    14,
  ),
  moon: icon(`<path d="M13.1 9.7A5.5 5.5 0 0 1 6.3 2.9a5.6 5.6 0 1 0 6.8 6.8z"/>`, 14),
  close: icon(`<path d="M4.2 4.2l7.6 7.6M11.8 4.2l-7.6 7.6"/>`, 14),
  folder: icon(
    `<path d="M2.3 5.4A1.5 1.5 0 0 1 3.8 3.9h2.1l1.3 1.6h5a1.5 1.5 0 0 1 1.5 1.5v4a1.5 1.5 0 0 1-1.5 1.5H3.8a1.5 1.5 0 0 1-1.5-1.5z"/>`,
    14,
  ),
};

export interface SettingsOptions {
  /** Current save folder, or null while it is still unresolved. */
  getSaveDir(): string | null;
  /** Open the native folder picker; resolves to the new folder (or null). */
  changeSaveDir(): Promise<string | null>;
  /** Version string for the footer, when the shell has one. */
  getVersion(): string | null;
  /** Called when the user picks a theme inside the panel. */
  setTheme(theme: Theme): void;
}

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

  /** Re-render current folder / version / theme (cheap, idempotent). */
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
  }

  /* ------------------------------------------------------------- internals */

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
            <p class="modal-desc" id="settings-desc">Where captures live, and how Capicorn looks.</p>
          </div>
          <button type="button" class="icon-btn modal-x" data-close aria-label="Close settings" title="Close (Esc)">${ICONS.close}</button>
        </header>
        <div class="modal-body">
          <section class="setting">
            <h3 class="setting-title">Save location</h3>
            <p class="setting-help">Every capture is saved here automatically. Edited copies are written to the same folder.</p>
            <div class="dir-row">
              <span class="dir-icon" aria-hidden="true">${ICONS.folder}</span>
              <code class="dir-path" id="settings-dir">Not set yet</code>
              <button type="button" class="btn-secondary" id="settings-change">Change…</button>
            </div>
          </section>
          <section class="setting">
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

    // Click on the scrim (not the card) closes.
    root.addEventListener("pointerdown", (ev) => {
      if (ev.target === root) this.close();
    });

    root.addEventListener("keydown", this.onKeyDown);
    document.body.append(root);
    return root;
  }

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
      if (!this.segments.includes(document.activeElement as HTMLButtonElement)) return;
      ev.preventDefault();
      const index = this.segments.findIndex((s) => s === document.activeElement);
      const step = ev.key === "ArrowRight" ? 1 : -1;
      const next = this.segments[(index + step + this.segments.length) % this.segments.length];
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