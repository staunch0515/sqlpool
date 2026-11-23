import type { PointerEvent as ReactPointerEvent } from "react";

// Drag behaviour follows the Vioros app: pointer capture on window, clamped sizes, saved on release.

export function clamp(n: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, n));
}

/** Reads a saved size, falling back to the default and clamping to the allowed range. */
export function loadNumber(key: string, def: number, min: number, max: number): number {
  try {
    const n = Number(localStorage.getItem(key));
    if (Number.isFinite(n) && n > 0) return clamp(n, min, max);
  } catch {
    /* storage unavailable */
  }
  return def;
}

export function saveNumber(key: string, n: number) {
  try {
    localStorage.setItem(key, String(n));
  } catch {
    /* storage unavailable */
  }
}

/**
 * Drag from a pointer press. `onMove` gets the distance moved since the press (x or y).
 * While dragging, the body gets a class that sets the cursor and blocks text selection.
 */
export function beginDrag(
  e: ReactPointerEvent,
  axis: "x" | "y",
  onMove: (delta: number) => void,
  onEnd: () => void,
) {
  e.preventDefault();
  e.stopPropagation();
  const origin = axis === "x" ? e.clientX : e.clientY;
  document.body.classList.add(axis === "x" ? "resizing" : "resizing-y");
  const move = (ev: PointerEvent) => onMove((axis === "x" ? ev.clientX : ev.clientY) - origin);
  const up = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    document.body.classList.remove("resizing", "resizing-y");
    onEnd();
  };
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
}

/** A draggable divider. axis "x" is vertical (drags left and right), "y" is horizontal. */
export function Splitter(props: { axis: "x" | "y"; onPointerDown: (e: ReactPointerEvent) => void }) {
  return (
    <div
      className={props.axis === "x" ? "splitter-v" : "splitter-h"}
      role="separator"
      aria-orientation={props.axis === "x" ? "vertical" : "horizontal"}
      onPointerDown={props.onPointerDown}
    />
  );
}
