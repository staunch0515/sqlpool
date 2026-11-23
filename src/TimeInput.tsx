import { useEffect, useRef, useState } from "react";
import DatePicker from "react-datepicker";
import "react-datepicker/dist/react-datepicker.css";

/** Parses "YYYY-MM-DDTHH:mm[:ss]" (UTC). The picker works on the same wall-clock numbers. */
function parseUtc(v: string): Date | null {
  const m = v.trim().match(/^(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2})(?::(\d{2}))?/);
  if (!m) return null;
  return new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5], m[6] ? +m[6] : 0);
}

/** Wall-clock text in UTC, e.g. "2026-11-10T14:57:00". The SQL side appends the Z. */
function toUtcText(d: Date): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
}

/** The UTC wall clock of a moment, as a Date the picker can show. */
function utcWall(ms: number): Date {
  const u = new Date(ms);
  return new Date(u.getUTCFullYear(), u.getUTCMonth(), u.getUTCDate(), u.getUTCHours(), u.getUTCMinutes(), u.getUTCSeconds());
}

function startOfUtcDay(ms: number): number {
  const d = new Date(ms);
  return Date.UTC(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate());
}

const HOUR = 3_600_000;
const DAY = 24 * HOUR;

const QUICK: { label: string; at: () => number }[] = [
  { label: "Now", at: () => Date.now() },
  { label: "Today 00:00", at: () => startOfUtcDay(Date.now()) },
  { label: "−1 h", at: () => Date.now() - HOUR },
  { label: "−1 d", at: () => Date.now() - DAY },
  { label: "−7 d", at: () => Date.now() - 7 * DAY },
  { label: "−30 d", at: () => Date.now() - 30 * DAY },
];

/**
 * Date and time picker for temporal parameters. Times are UTC. Type a value or pick it from the
 * calendar; the quick buttons set common times. Empty means "not given" (NULL, or now for As of).
 */
export function TimeInput(props: { value: string; onChange: (value: string) => void; placeholder?: string }) {
  const { value, onChange } = props;

  // The calendar pops out of the pane, so it lives in its own layer on the page.
  useEffect(() => {
    if (!document.getElementById("picker-layer")) {
      const layer = document.createElement("div");
      layer.id = "picker-layer";
      document.body.appendChild(layer);
    }
  }, []);

  // The quick-pick remembers what was picked, until the time is typed or picked on the calendar.
  const [picked, setPicked] = useState("");

  function set(d: Date | null) {
    onChange(d ? toUtcText(d) : "");
  }

  function typed(d: Date | null) {
    setPicked("");
    set(d);
  }

  return (
    <div className="time-input">
      <DatePicker
        selected={parseUtc(value)}
        onChange={(d: Date | null) => typed(d)}
        showTimeSelect
        timeIntervals={1}
        timeFormat="HH:mm:ss"
        dateFormat="yyyy-MM-dd HH:mm:ss"
        timeCaption="UTC"
        placeholderText={props.placeholder ?? "yyyy-mm-dd hh:mm:ss (UTC)"}
        isClearable
        todayButton="Today"
        portalId="picker-layer"
        popperPlacement="bottom-start"
        showMonthDropdown
        showYearDropdown
        dropdownMode="select"
        className="time-text"
        wrapperClassName="time-wrap"
      />
      <QuickPick
        label={picked === "" ? "Quick…" : QUICK[Number(picked)]?.label ?? "Quick…"}
        picked={picked}
        onPick={(i) => {
          const q = QUICK[i];
          if (!q) return;
          setPicked(String(i));
          set(utcWall(q.at()));
        }}
      />
    </div>
  );
}

/**
 * The quick-pick: a square button that opens a menu just below it (not a native select, whose menu
 * opens over the button). A click outside or a choice closes it.
 */
function QuickPick(props: { label: string; picked: string; onPick: (index: number) => void }) {
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
    <div className="time-quick" ref={box}>
      <button type="button" className="time-pick" title="Quick pick" aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        {props.label}
      </button>
      {open && (
        <div className="time-menu" role="listbox">
          {QUICK.map((q, i) => (
            <div
              key={q.label}
              role="option"
              aria-selected={props.picked === String(i)}
              className={props.picked === String(i) ? "time-menu-item active" : "time-menu-item"}
              onClick={() => {
                props.onPick(i);
                setOpen(false);
              }}
            >
              {q.label}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
