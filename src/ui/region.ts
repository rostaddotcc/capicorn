// Region selection overlay + canvas crop.
// Pure DOM/canvas — no editor or Tauri dependency. Boxes are in NATURAL image pixels.

export type Box = { left: number; top: number; width: number; height: number };

const ACCENT = "#8b7cf6";
/** Below this (in natural px) a drag is not considered a real selection. */
const MIN_BOX = 3;

type Fit = { ok: true; left: number; top: number; width: number; height: number; scale: number };

function fitInto(nw: number, nh: number, cw: number, ch: number): Fit {
  const scale = Math.min(cw / nw, ch / nh);
  const width = Math.round(nw * scale);
  const height = Math.round(nh * scale);
  return {
    ok: true,
    left: Math.round((cw - width) / 2),
    top: Math.round((ch - height) / 2),
    width,
    height,
    scale,
  };
}

function loadImage(src: string): Promise<HTMLImageElement> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.decoding = "async";
    img.onload = () => resolve(img);
    img.onerror = () => reject(new Error("Could not decode the screenshot for region capture."));
    img.src = src;
  });
}

const clamp = (v: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, v));

/**
 * Show a fit-to-viewport screenshot in a FIXED, viewport-covering overlay with a
 * crosshair drag selector. Call this while the app window is fullscreen so the
 * frozen screenshot maps ~1:1 onto the real screen.
 * Resolves the selection rectangle in NATURAL image pixels, or null when cancelled.
 *
 * @param _host kept for call-site compatibility; the overlay always mounts on <body>.
 */
export function selectRegion(dataUrl: string, _host?: HTMLElement): Promise<Box | null> {
  return new Promise<Box | null>((resolve) => {
    const overlay = document.createElement("div");
    overlay.className = "region-overlay";
    overlay.tabIndex = -1;

    const img = document.createElement("img");
    img.className = "region-image";
    img.alt = "";
    img.draggable = false;
    img.src = dataUrl;

    const shadeBox = document.createElement("div");
    shadeBox.className = "region-selection";
    shadeBox.hidden = true;

    const label = document.createElement("span");
    label.className = "region-label";
    label.hidden = true;

    const hint = document.createElement("div");
    hint.className = "region-hint";
    hint.innerHTML =
      "<span><strong>Drag</strong> to select a region of the screen</span><span class=\"region-hint-keys\"><kbd>Enter</kbd> confirm · <kbd>Esc</kbd> cancel</span>";

    document.querySelectorAll(".region-overlay").forEach((el) => el.remove());
    overlay.append(img, shadeBox, label, hint);
    document.body.appendChild(overlay);

    let natural = { w: 0, h: 0 };
    let rect: Fit | null = null;
    let dragging = false;
    let startX = 0;
    let startY = 0;
    let box: Box | null = null;
    let done = false;

    const overlayPoint = (ev: PointerEvent | MouseEvent): { x: number; y: number } => {
      const r = overlay.getBoundingClientRect();
      return { x: ev.clientX - r.left, y: ev.clientY - r.top };
    };

    const toNatural = (x: number, y: number): { x: number; y: number } => {
      if (!rect) return { x: 0, y: 0 };
      return {
        x: clamp((x - rect.left) / rect.scale, 0, natural.w),
        y: clamp((y - rect.top) / rect.scale, 0, natural.h),
      };
    };

    const paint = (b: Box | null): void => {
      box = b;
      if (!b || !rect) {
        shadeBox.hidden = true;
        label.hidden = true;
        return;
      }
      const sl = rect.left + b.left * rect.scale;
      const st = rect.top + b.top * rect.scale;
      const sw = b.width * rect.scale;
      const sh = b.height * rect.scale;
      shadeBox.hidden = false;
      shadeBox.style.left = `${sl}px`;
      shadeBox.style.top = `${st}px`;
      shadeBox.style.width = `${sw}px`;
      shadeBox.style.height = `${sh}px`;
      label.hidden = false;
      label.textContent = `${Math.round(b.width)} × ${Math.round(b.height)}`;
      const labelTop = st > 26 ? st - 24 : st + sh + 8;
      label.style.left = `${clamp(sl, 0, Math.max(0, overlay.clientWidth - 78))}px`;
      label.style.top = `${Math.max(0, labelTop)}px`;
    };

    let onResize: () => void = () => {};

    const cleanup = (value: Box | null): void => {
      if (done) return;
      done = true;
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("resize", onResize);
      overlay.remove();
      resolve(value);
    };

    const cancel = (): void => cleanup(null);

    const commit = (): void => {
      if (!box || box.width < MIN_BOX || box.height < MIN_BOX) return;
      cleanup({
        left: Math.round(box.left),
        top: Math.round(box.top),
        width: Math.max(1, Math.round(box.width)),
        height: Math.max(1, Math.round(box.height)),
      });
    };

    const onDown = (ev: PointerEvent): void => {
      if (ev.button !== 0 || !rect) return;
      ev.preventDefault();
      const p = overlayPoint(ev);
      // The overlay spans the whole (fullscreen) window, so a drag may start on the
      // letterbox area too — coordinates are clamped to the image in toNatural().
      dragging = true;
      try {
        overlay.setPointerCapture(ev.pointerId);
      } catch {
        /* capture is best-effort; window-level listeners are not needed */
      }
      startX = p.x;
      startY = p.y;
      hint.hidden = true;
      paint({ left: 0, top: 0, width: 0, height: 0 });
    };

    const onMove = (ev: PointerEvent): void => {
      if (!dragging || !rect) return;
      const p = overlayPoint(ev);
      const a = toNatural(Math.min(startX, p.x), Math.min(startY, p.y));
      const b = toNatural(Math.max(startX, p.x), Math.max(startY, p.y));
      paint({
        left: a.x,
        top: a.y,
        width: Math.max(0, b.x - a.x),
        height: Math.max(0, b.y - a.y),
      });
    };

    const onUp = (ev: PointerEvent): void => {
      if (!dragging) return;
      dragging = false;
      try {
        if (overlay.hasPointerCapture(ev.pointerId)) overlay.releasePointerCapture(ev.pointerId);
      } catch {
        /* already released */
      }
      if (box && box.width >= MIN_BOX && box.height >= MIN_BOX) commit();
      else paint(null);
    };

    const onKey = (ev: KeyboardEvent): void => {
      if (ev.key === "Escape") {
        ev.preventDefault();
        ev.stopPropagation();
        cancel();
      } else if (ev.key === "Enter") {
        ev.preventDefault();
        ev.stopPropagation();
        commit();
      }
    };

    onResize = (): void => {
      if (!natural.w || !natural.h) return;
      rect = fitInto(natural.w, natural.h, overlay.clientWidth, overlay.clientHeight);
      img.style.left = `${rect.left}px`;
      img.style.top = `${rect.top}px`;
      img.style.width = `${rect.width}px`;
      img.style.height = `${rect.height}px`;
      paint(box);
    };

    overlay.addEventListener("pointerdown", onDown);
    overlay.addEventListener("pointermove", onMove);
    overlay.addEventListener("pointerup", onUp);
    overlay.addEventListener("pointercancel", () => {
      dragging = false;
      paint(null);
    });
    overlay.addEventListener("contextmenu", (ev) => {
      ev.preventDefault();
      cancel();
    });
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("resize", onResize);

    const ready = (): void => {
      natural = { w: img.naturalWidth, h: img.naturalHeight };
      onResize();
      if (!natural.w || !natural.h) cancel();
      overlay.classList.add("is-ready");
      overlay.focus({ preventScroll: true });
    };

    if (img.complete && img.naturalWidth) ready();
    else img.addEventListener("load", ready, { once: true });
    img.addEventListener(
      "error",
      () => cancel(),
      { once: true },
    );
  });
}

/** Draw a natural-resolution sub-rect of `dataUrl` to a canvas and return a PNG dataUrl. */
export async function cropDataUrl(dataUrl: string, box: Box): Promise<string> {
  const img = await loadImage(dataUrl);
  const left = clamp(Math.round(box.left), 0, Math.max(0, img.naturalWidth - 1));
  const top = clamp(Math.round(box.top), 0, Math.max(0, img.naturalHeight - 1));
  const width = clamp(Math.round(box.width), 1, img.naturalWidth - left);
  const height = clamp(Math.round(box.height), 1, img.naturalHeight - top);

  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("Canvas 2D context unavailable for cropping.");
  ctx.drawImage(img, left, top, width, height, 0, 0, width, height);
  return canvas.toDataURL("image/png");
}

/** Exposed for tests / future UI affordances. */
export const REGION_ACCENT = ACCENT;