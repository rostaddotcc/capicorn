// Capicorn — floating quick-access widget.
//
// A second, frameless window (~260x64) that sits above the desktop and drives
// the main window: "Full" / "Region" start a capture there, the × hides this
// widget again. Everything is built here so the window boots from one module;
// the shell markup is deliberately empty.
//
// Backend contract (src-tauri):
//   invoke('widget_capture', { mode: 'full' | 'region' }) — start a capture
//   invoke('close_widget')                                — hide this window

import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./widget.css";

const THEME_KEY = "capicorn:theme";

type Theme = "light" | "dark";
type CaptureMode = "full" | "region";

/* ------------------------------------------------------------------ theme */

function readTheme(): Theme {
  try {
    return localStorage.getItem(THEME_KEY) === "light" ? "light" : "dark";
  } catch {
    return "dark";
  }
}

function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
}

/* ------------------------------------------------------------------ icons */

const svg = (body: string): string =>
  `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" ` +
  `stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" focusable="false">${body}</svg>`;

const ICON_FULL = svg(
  `<rect x="2.5" y="4" width="19" height="12.5" rx="1.8"/>` +
    `<path d="M8.5 20.5h7"/><path d="M12 16.5v4"/>`
);

const ICON_REGION = svg(
  `<path d="M6.5 2.5v15a2 2 0 0 0 2 2h13"/>` +
    `<path d="M2.5 6.5h15a2 2 0 0 1 2 2v13"/>`
);

const ICON_CLOSE = svg(`<path d="M6.5 6.5l11 11"/><path d="M17.5 6.5l-11 11"/>`);

/* ------------------------------------------------------------------- build */

function button(opts: {
  id: string;
  label: string;
  icon: string;
  tip: string;
  variant?: "ghost";
}): HTMLButtonElement {
  const el = document.createElement("button");
  el.type = "button";
  el.id = opts.id;
  el.className = opts.variant === "ghost" ? "w-btn w-btn--ghost" : "w-btn";
  el.title = opts.tip;
  el.setAttribute("aria-label", opts.tip);

  const glyph = document.createElement("span");
  glyph.className = "w-icon";
  glyph.innerHTML = opts.icon;

  const text = document.createElement("span");
  text.className = "w-label";
  text.textContent = opts.label;

  el.append(glyph, text);
  return el;
}

function mount(): void {
  const host = document.getElementById("widget");
  if (!host) return;
  const root: HTMLElement = host;

  root.classList.add("w-bar");
  root.setAttribute("role", "toolbar");
  root.setAttribute("aria-label", "Capicorn quick capture");

  // Drag grip — the frameless window has no titlebar, so this is the handle.
  const grip = document.createElement("div");
  grip.className = "w-grip";
  grip.setAttribute("title", "Drag to move");
  grip.setAttribute("aria-hidden", "true");
  const dots = document.createElement("span");
  dots.className = "w-grip__grid";
  for (let i = 0; i < 6; i += 1) {
    const dot = document.createElement("i");
    dot.className = "w-grip__dot";
    dots.append(dot);
  }
  grip.append(dots);

  const full = button({
    id: "w-full",
    label: "Full",
    icon: ICON_FULL,
    tip: "Capture full screen (F)",
  });

  const region = button({
    id: "w-region",
    label: "Region",
    icon: ICON_REGION,
    tip: "Capture a region (R)",
  });

  const sep = document.createElement("span");
  sep.className = "w-sep";
  sep.setAttribute("aria-hidden", "true");

  const close = button({
    id: "w-close",
    label: "",
    icon: ICON_CLOSE,
    tip: "Close widget (Esc)",
    variant: "ghost",
  });
  close.classList.add("w-btn--close");

  root.append(grip, full, region, sep, close);

  // Move the frameless window: dragging anywhere on the pill (except the
  // buttons) starts an interactive compositor move. startDragging() maps to
  // xdg_toplevel.move on Wayland, which KDE/GNOME honour — unlike manual
  // set_position, which Wayland ignores. The whole pill is the handle, so it
  // is easy to grab; the buttons remain ordinary clickable controls.
  const win = getCurrentWindow();
  root.addEventListener("mousedown", (ev) => {
    if (ev.button !== 0) return;
    const target = ev.target as HTMLElement | null;
    if (target && target.closest("button")) return;
    ev.preventDefault();
    void win.startDragging().catch(() => {
      /* the compositor refused the move — nothing further to try */
    });
  });

  /* ---------------------------------------------------------------- wiring */

  // A short debounce kept as a timestamp, not a timer: the widget window is
  // small and often unfocused, and background throttling would otherwise leave
  // a boolean stuck "busy" and swallow the next click.
  let busyUntil = 0;

  function flash(el: HTMLButtonElement): void {
    if (typeof el.animate !== "function") return;
    el.animate(
      [
        { transform: "scale(0.94)", backgroundColor: "var(--accent-press)" },
        { transform: "scale(1)", backgroundColor: "var(--accent-soft)" },
        { transform: "scale(1)", backgroundColor: "transparent" },
      ],
      { duration: 320, easing: "ease-out" }
    );
  }

  async function capture(mode: CaptureMode, el: HTMLButtonElement): Promise<void> {
    const now = performance.now();
    if (now < busyUntil) return;
    busyUntil = now + 400;

    flash(el);
    try {
      await invoke("widget_capture", { mode });
    } catch (err) {
      console.warn("[widget] widget_capture failed", err);
    }
  }

  async function closeWidget(): Promise<void> {
    try {
      await invoke("close_widget");
    } catch (err) {
      console.warn("[widget] close_widget failed", err);
    }
  }

  full.addEventListener("click", () => void capture("full", full));
  region.addEventListener("click", () => void capture("region", region));
  close.addEventListener("click", () => void closeWidget());

  document.addEventListener("keydown", (ev) => {
    if (ev.key === "Escape") {
      ev.preventDefault();
      void closeWidget();
      return;
    }
    if (ev.metaKey || ev.ctrlKey || ev.altKey) return;

    const key = ev.key.toLowerCase();
    if (key === "f") {
      ev.preventDefault();
      void capture("full", full);
    } else if (key === "r") {
      ev.preventDefault();
      void capture("region", region);
    }
  });

  // Keep the widget in step when the app flips the theme while it is open.
  window.addEventListener("storage", (ev) => {
    if (ev.key === THEME_KEY) applyTheme(readTheme());
  });
}

applyTheme(readTheme());
if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", mount, { once: true });
} else {
  mount();
}