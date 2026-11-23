import { useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { beginDrag } from "./resize";
import type { QueryResult } from "./types";

const DEFAULT_COL = 150;
const COL_MIN = 60;

/** Collapsible block. Closed by default unless `open` is set. */
export function Section(props: {
  title: string;
  count?: number;
  open?: boolean;
  children: ReactNode;
}) {
  return (
    <details className="section" open={props.open}>
      <summary>
        <span>{props.title}</span>
        {props.count !== undefined && <span className="count-badge">{props.count}</span>}
      </summary>
      <div className="section-body">{props.children}</div>
    </details>
  );
}

export function Banner(props: { kind: "error" | "warn" | "ok"; children: ReactNode }) {
  return <div className={`banner ${props.kind}`}>{props.children}</div>;
}

/**
 * Result rows, in the Vioros table style: click a header to sort (asc, desc, off), drag a header's
 * right edge to resize a column, scroll sideways when there are many columns. The header stays on top.
 */
export function ResultTable(props: { result: QueryResult }) {
  const { result } = props;
  const colKey = result.columns.join("\u0001");
  const [widths, setWidths] = useState<number[]>(() => result.columns.map(() => DEFAULT_COL));
  const [sort, setSort] = useState<{ col: number; dir: "asc" | "desc" } | null>(null);

  // A new result with other columns starts with default widths and no sort.
  useEffect(() => {
    setWidths(result.columns.map(() => DEFAULT_COL));
    setSort(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [colKey]);

  const rows = useMemo(() => {
    if (!sort) return result.rows;
    const sign = sort.dir === "asc" ? 1 : -1;
    return [...result.rows].sort((a, b) => {
      const x = a[sort.col];
      const y = b[sort.col];
      if (x === null && y === null) return 0;
      if (x === null) return 1; // NULLs last, whichever way
      if (y === null) return -1;
      const nx = Number(x);
      const ny = Number(y);
      if (x.trim() !== "" && y.trim() !== "" && Number.isFinite(nx) && Number.isFinite(ny)) return (nx - ny) * sign;
      return x.localeCompare(y, undefined, { numeric: true }) * sign;
    });
  }, [result.rows, sort]);

  function clickHeader(col: number) {
    if (!sort || sort.col !== col) setSort({ col, dir: "asc" });
    else if (sort.dir === "asc") setSort({ col, dir: "desc" });
    else setSort(null);
  }

  function startResize(col: number) {
    return (e: ReactPointerEvent) => {
      const start = widths[col];
      let latest = start;
      beginDrag(
        e,
        "x",
        (d) => {
          latest = Math.max(COL_MIN, start + d);
          setWidths((prev) => prev.map((w, i) => (i === col ? latest : w)));
        },
        () => undefined,
      );
    };
  }

  const tableWidth = widths.reduce((sum, w) => sum + w, 0);

  return (
    <>
      <div className="table-wrap">
        <table className="result-table" style={{ minWidth: tableWidth }}>
          <colgroup>
            {widths.map((w, i) => (
              <col key={i} style={{ width: w }} />
            ))}
          </colgroup>
          <thead>
            <tr>
              {result.columns.map((c, i) => {
                const active = sort?.col === i;
                return (
                  <th key={`${c}-${i}`} className="sortable" onClick={() => clickHeader(i)} aria-sort={active ? (sort?.dir === "asc" ? "ascending" : "descending") : "none"}>
                    <span className="th-label">{c}</span>
                    {active ? (sort?.dir === "asc" ? " ▲" : " ▼") : ""}
                    <span
                      className="col-resizer"
                      onPointerDown={startResize(i)}
                      onClick={(e) => e.stopPropagation()}
                      role="separator"
                      aria-orientation="vertical"
                    />
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {rows.map((row, r) => (
              <tr key={r}>
                {row.map((cell, j) => (
                  <td key={j} title={cell ?? "NULL"}>
                    {cell ?? <i className="null">NULL</i>}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </>
  );
}

/** Sub-tabs under the path bar (Vioros: List / Download / Upload). The active tab has an accent underline. */
export function SubTabs(props: {
  tabs: { id: string; label: string; count?: number }[];
  active: string;
  onChange: (id: string) => void;
}) {
  return (
    <div className="sub-tabs" role="tablist">
      {props.tabs.map((t) => (
        <button
          key={t.id}
          role="tab"
          aria-selected={t.id === props.active}
          className={t.id === props.active ? "sub-tab active" : "sub-tab"}
          onClick={() => props.onChange(t.id)}
        >
          {t.label}
          {t.count !== undefined && <span className="count-badge">{t.count}</span>}
        </button>
      ))}
    </div>
  );
}

/** A small rotating ring: "this is working". */
export function Spinner(props: { label?: string }) {
  return (
    <span className="working" role="status">
      <span className="spinner" />
      {props.label && <span>{props.label}</span>}
    </span>
  );
}

/** Square icon button for toolbars. The title shows on hover. While busy it shows a spinner instead. */
export function IconButton(props: {
  icon: ReactNode;
  title: string;
  onClick: () => void;
  active?: boolean;
  disabled?: boolean;
  busy?: boolean;
}) {
  return (
    <button
      className={props.active ? "icon-sq active" : "icon-sq"}
      title={props.busy ? `${props.title} (working…)` : props.title}
      aria-label={props.title}
      onClick={props.onClick}
      disabled={props.disabled || props.busy}
    >
      {props.busy ? <span className="spinner" /> : props.icon}
    </button>
  );
}

/** Search box at the top of a side panel (Termix "Search hosts"). */
export function SideSearch(props: { value: string; onChange: (value: string) => void; placeholder: string }) {
  return (
    <div className="side-search">
      <input value={props.value} placeholder={props.placeholder} onChange={(e) => props.onChange(e.target.value)} />
    </div>
  );
}

/**
 * Covers an area while something runs: a dimmed layer with a big spinner and a timer. The whole area
 * is visibly busy, not just a small icon.
 */
export function BusyOverlay(props: { label: string }) {
  const [elapsed, setElapsed] = useState(0);
  useEffect(() => {
    const started = Date.now();
    const t = setInterval(() => setElapsed((Date.now() - started) / 1000), 100);
    return () => clearInterval(t);
  }, []);
  return (
    <div className="busy-overlay" role="status" aria-live="polite">
      <div className="busy-card">
        <span className="spinner huge" />
        <div>
          <div className="busy-title">{props.label}</div>
          <div className="busy-time">{elapsed.toFixed(1)} s</div>
        </div>
      </div>
    </div>
  );
}

/**
 * A short message (saved, error) that floats over the corner of its pane. It never takes up room,
 * so nothing below or above it moves. It goes away by itself after a few seconds, or on a click.
 */
export function Toast(props: { kind: "ok" | "error"; text: string | null; onDismiss: () => void }) {
  const { kind, text, onDismiss } = props;
  useEffect(() => {
    if (!text) return;
    const t = setTimeout(onDismiss, 4000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text]);
  if (!text) return null;
  return (
    <div
      className={kind === "error" ? "panel-toast error" : "panel-toast"}
      role="status"
      onClick={onDismiss}
      title="Click to dismiss"
    >
      {text}
    </div>
  );
}

/**
 * A square button that opens a menu just below it, the same look as the quick-pick. Used instead of a
 * native select, whose menu opens over the button and looks different on every system.
 */
export function PickMenu(props: {
  label: string;
  items: { key: string; label: string; active: boolean; onPick: () => void }[];
}) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const close = (e: MouseEvent) => {
      if (box.current && !box.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    return () => document.removeEventListener("mousedown", close);
  }, [open]);
  return (
    <div className="pick" ref={box}>
      <button type="button" className="pick-button" aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {props.label}
      </button>
      {open && (
        <div className="pick-menu" role="listbox">
          {props.items.map((it) => (
            <div
              key={it.key}
              role="option"
              aria-selected={it.active}
              className={it.active ? "pick-item active" : "pick-item"}
              onClick={() => {
                it.onPick();
                setOpen(false);
              }}
            >
              {it.label}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
