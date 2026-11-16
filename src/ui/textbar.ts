// SnagCraft — floating quick text settings.
//
// A compact bar that hugs the caption being edited or selected: font family,
// size −/+, bold, italic and colour — right on the image, no trip to the
// properties rail. Tauri-free on purpose (same rule as the settings panel):
// everything native arrives through the small bridge below, so the bar is
// testable in a plain browser.
//
// Lifecycle: shown while a text object is editing or selected; once shown it
// keeps its target until the object leaves the scene or the user clicks back
// into the canvas without an active text. Buttons preventDefault on
// pointerdown so typing focus survives a bold/size/color click; the font
// <select> and colour picker need real focus, which settles the caption —
// the bar then keeps styling the committed box.

/** The slice of a fabric text object the bar touches (structural, no import). */
export interface TextTarget {
  fontFamily?: string;
  fontSize?: number;
  fontWeight?: string | number;
  fontStyle?: string;
  fill?: unknown;
  isEditing?: boolean;
  dirty?: boolean;
  set(props: Record<string, unknown>): void;
  getBoundingRect(): { left: number; top: number; width: number; height: number };
}

export interface TextBarFont {
  family: string;
  css: string;
  stack: string;
}

export interface TextBarBridge {
  /** The caption being edited or selected, if any. */
  getActiveText(): TextTarget | null;
  /** Upper canvas element — the coordinate anchor. */
  getOverlayAnchor(): HTMLCanvasElement | null;
  /** Is the object still in the scene? */
  hasObject(object: TextTarget): boolean;
  /** Render/selection/editing subscription; returns unsubscribe. */
  subscribe(callback: () => void): () => void;
  /** Record a direct style edit in history. */
  commit(): void;
  /** Keep the rail + new-text defaults in step with the bar. */
  setFontFamily(family: string): void;
  setFontSize(size: number): void;
  setBold(bold: boolean): void;
  setItalic(italic: boolean): void;
  setColor(color: string): void;
}

export interface TextBarOptions {
  /** Positioned ancestor the bar is absolutely placed inside (the stage). */
  host: HTMLElement;
  fonts: ReadonlyArray<TextBarFont>;
  bridge: TextBarBridge;
  minSize: number;
  maxSize: number;
}

const isBold = (target: TextTarget): boolean =>
  target.fontWeight === "bold" || Number(target.fontWeight) >= 600;

/** fabric fills can be gradient objects; the bar only edits solid colours. */
const solidFill = (target: TextTarget): string | null => {
  const fill = target.fill;
  if (typeof fill !== "string") return null;
  const hex = /^#([0-9a-f]{6})$/i.exec(fill.trim());
  if (hex) return `#${hex[1].toLowerCase()}`;
  const short = /^#([0-9a-f]{3})$/i.exec(fill.trim());
  if (short) {
    const [r, g, b] = short[1].split("");
    return `#${r}${r}${g}${g}${b}${b}`.toLowerCase();
  }
  const rgb = /^rgba?\(\s*(\d+)[,\s]+(\d+)[,\s]+(\d+)/.exec(fill.trim());
  if (rgb) {
    const to2 = (n: string) => Number(n).toString(16).padStart(2, "0");
    return `#${to2(rgb[1])}${to2(rgb[2])}${to2(rgb[3])}`;
  }
  return null;
};

export class TextBar {
  private readonly opts: TextBarOptions;
  private readonly root: HTMLDivElement;
  private readonly fontSelect: HTMLSelectElement;
  private readonly sizeOut: HTMLOutputElement;
  private readonly boldBtn: HTMLButtonElement;
  private readonly italicBtn: HTMLButtonElement;
  private readonly colorInput: HTMLInputElement;
  private target: TextTarget | null = null;
  private unsubscribe: (() => void) | null = null;
  private disposed = false;

  constructor(opts: TextBarOptions) {
    this.opts = opts;

    const root = document.createElement("div");
    root.className = "textbar";
    root.hidden = true;
    root.setAttribute("role", "toolbar");
    root.setAttribute("aria-label", "Text settings");

    const font = document.createElement("select");
    font.className = "textbar-font";
    font.setAttribute("aria-label", "Font family");
    font.title = "Font";
    for (const entry of opts.fonts) {
      const option = document.createElement("option");
      option.value = entry.family;
      option.textContent = entry.family;
      option.style.fontFamily = entry.stack;
      font.append(option);
    }

    const minus = this.button("textbar-step", "−", "Smaller text");
    const size = document.createElement("output");
    size.className = "textbar-size";
    size.setAttribute("aria-live", "off");
    const plus = this.button("textbar-step", "+", "Larger text");

    const bold = this.button("textbar-toggle", "B", "Bold");
    bold.classList.add("textbar-bold");
    const italic = this.button("textbar-toggle", "I", "Italic");
    italic.classList.add("textbar-italic");

    const color = document.createElement("input");
    color.type = "color";
    color.className = "textbar-color";
    color.setAttribute("aria-label", "Text colour");
    color.title = "Text colour";
    color.value = "#1f2430";

    root.append(font, minus, size, plus, bold, italic, color);
    opts.host.append(root);

    this.root = root;
    this.fontSelect = font;
    this.sizeOut = size;
    this.boldBtn = bold;
    this.italicBtn = italic;
    this.colorInput = color;

    /* ----------------------------------------------------------- events */

    // Buttons must not steal the caret from a live caption.
    for (const btn of [minus, plus, bold, italic]) {
      btn.addEventListener("pointerdown", (ev) => ev.preventDefault());
    }

    font.addEventListener("change", () => {
      const target = this.target;
      if (!target) return;
      const entry = opts.fonts.find((f) => f.family === font.value);
      if (!entry) return;
      target.set({ fontFamily: entry.family });
      target.dirty = true;
      opts.bridge.setFontFamily(entry.family);
      opts.bridge.commit();
      this.sync();
    });

    const step = (delta: number): void => {
      const target = this.target;
      if (!target) return;
      const current = Number(target.fontSize) || opts.minSize;
      const next = Math.min(Math.max(current + delta, opts.minSize), opts.maxSize);
      target.set({ fontSize: next });
      target.dirty = true;
      opts.bridge.setFontSize(next);
      opts.bridge.commit();
      this.sync();
    };
    minus.addEventListener("click", () => step(-2));
    plus.addEventListener("click", () => step(2));

    bold.addEventListener("click", () => {
      const target = this.target;
      if (!target) return;
      const next = !isBold(target);
      target.set({ fontWeight: next ? "bold" : "normal" });
      target.dirty = true;
      opts.bridge.setBold(next);
      opts.bridge.commit();
      this.sync();
    });

    italic.addEventListener("click", () => {
      const target = this.target;
      if (!target) return;
      const next = target.fontStyle !== "italic";
      target.set({ fontStyle: next ? "italic" : "normal" });
      target.dirty = true;
      opts.bridge.setItalic(next);
      opts.bridge.commit();
      this.sync();
    });

    color.addEventListener("input", () => {
      const target = this.target;
      if (!target) return;
      target.set({ fill: color.value });
      target.dirty = true;
      opts.bridge.setColor(color.value);
      // Continuous drag: commit once when the picker settles, not per pixel.
    });
    color.addEventListener("change", () => {
      opts.bridge.commit();
      this.sync();
    });

    // A click back into the canvas (not on the bar) releases a held target
    // that is neither editing nor selected.
    opts.host.addEventListener("pointerdown", (ev) => {
      if (this.root.hidden) return;
      if (ev.target instanceof Node && this.root.contains(ev.target)) return;
      const active = opts.bridge.getActiveText();
      if (!active && this.target && !this.target.isEditing) this.hide();
    });

    this.unsubscribe = opts.bridge.subscribe(() => this.update());
    this.update();
  }

  private button(className: string, label: string, tip: string): HTMLButtonElement {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = className;
    btn.textContent = label;
    btn.title = tip;
    btn.setAttribute("aria-label", tip);
    return btn;
  }

  /* -------------------------------------------------------------- state */

  private update(): void {
    if (this.disposed) return;
    const active = this.opts.bridge.getActiveText();
    if (active) {
      this.target = active;
      this.root.hidden = false;
    } else if (this.target && !this.opts.bridge.hasObject(this.target)) {
      // The held box left the scene (discarded empty, deleted, undone).
      this.hide();
      return;
    }
    if (!this.target || this.root.hidden) return;
    this.position();
    this.sync();
  }

  private hide(): void {
    this.target = null;
    this.root.hidden = true;
  }

  /** Drop the held target (the shell calls this when the tool changes). */
  release(): void {
    if (!this.opts.bridge.getActiveText()) this.hide();
  }

  /** Place the bar above the target's bounding box, clamped to the stage. */
  private position(): void {
    const target = this.target;
    const anchor = this.opts.bridge.getOverlayAnchor();
    if (!target || !anchor) return;
    const hostRect = this.opts.host.getBoundingClientRect();
    const anchorRect = anchor.getBoundingClientRect();
    if (anchor.width === 0 || anchorRect.width === 0) return;
    const scale = anchorRect.width / anchor.width;
    const box = target.getBoundingRect();
    const barW = this.root.offsetWidth || 280;
    const barH = this.root.offsetHeight || 36;

    let left = anchorRect.left - hostRect.left + box.left * scale;
    let top = anchorRect.top - hostRect.top + box.top * scale - barH - 8;
    // No room above → below the box; never off the stage.
    if (top < 4) top = anchorRect.top - hostRect.top + (box.top + box.height) * scale + 8;
    left = Math.min(Math.max(left, 4), Math.max(hostRect.width - barW - 4, 4));
    top = Math.min(Math.max(top, 4), Math.max(hostRect.height - barH - 4, 4));
    this.root.style.left = `${Math.round(left)}px`;
    this.root.style.top = `${Math.round(top)}px`;
  }

  /** Mirror the target's style into the controls (never fight the caret). */
  private sync(): void {
    const target = this.target;
    if (!target) return;
    const active = document.activeElement;

    if (active !== this.fontSelect) {
      const family = String(target.fontFamily ?? "").split(",")[0].replace(/['"]/g, "").trim();
      const known = this.opts.fonts.some((f) => f.family === family);
      this.fontSelect.value = known ? family : this.opts.fonts[0]?.family ?? "";
      const entry = this.opts.fonts.find((f) => f.family === this.fontSelect.value);
      this.fontSelect.style.fontFamily = entry?.stack ?? "";
    }
    this.sizeOut.textContent = String(Math.round(Number(target.fontSize) || 0));
    this.boldBtn.classList.toggle("is-on", isBold(target));
    this.boldBtn.setAttribute("aria-pressed", String(isBold(target)));
    const italic = target.fontStyle === "italic";
    this.italicBtn.classList.toggle("is-on", italic);
    this.italicBtn.setAttribute("aria-pressed", String(italic));
    if (active !== this.colorInput) {
      const fill = solidFill(target);
      if (fill) this.colorInput.value = fill;
    }
  }

  destroy(): void {
    this.disposed = true;
    this.unsubscribe?.();
    this.unsubscribe = null;
    this.root.remove();
  }
}
