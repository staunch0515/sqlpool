import { useState, type ReactNode } from "react";
import { beginDrag, clamp, loadNumber, saveNumber, Splitter } from "./resize";
import { ChevronRight, Checklist, Clock, Copy, CopyPlus, Diff, Download, Hash, History, ListTree, Play, Save, Sparkles, Star, StarFilled, Table, Trash, X } from "./icons";

/** The frame of an editor tab, VS Code style: breadcrumb, title with a toolbar, then the body. */
export function EditorPane(props: {
  crumbs: string[];
  title: string;
  subtitle?: ReactNode;
  actions?: ReactNode;
  /** Controls at the right edge of the toolbar, right-aligned (e.g. a row limit). */
  trailing?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="pane">
      <nav className="breadcrumb" aria-label="Breadcrumb">
        {props.crumbs.map((c, i) => {
          const last = i === props.crumbs.length - 1;
          return (
            <span key={`${i}-${c}`} className={last ? "crumb-part current" : "crumb-part"}>
              {i > 0 && <ChevronRight size={12}  className="crumb-sep" />}
              <span>{c}</span>
            </span>
          );
        })}
      </nav>
      {/* the name is already in the breadcrumb: the toolbar holds only the icon buttons, on the right */}
      <div className="pane-toolbar">
        <div className="pane-actions">{props.actions}</div>
        {props.trailing && <div className="pane-trailing">{props.trailing}</div>}
      </div>
      {props.children}
    </div>
  );
}

/** The icons of the SQL toolbar: one set, one size. */
export type ToolIcon = "play" | "save" | "saveAs" | "copy" | "hash" | "star" | "starFilled" | "trash" | "x" | "tree" | "download" | "sparkles" | "table" | "history" | "clock" | "diff" | "checklist";

const ICONS = {
  play: Play,
  save: Save,
  saveAs: CopyPlus,
  copy: Copy,
  hash: Hash,
  star: Star,
  starFilled: StarFilled,
  trash: Trash,
  x: X,
  tree: ListTree,
  download: Download,
  table: Table,
  history: History,
  clock: Clock,
  diff: Diff,
  checklist: Checklist,
  sparkles: Sparkles,
};

/** A small icon-only toolbar button. The name shows on hover. Busy shows a small spinner instead. */
export function ToolButton(props: {
  icon: ToolIcon;
  label: string;
  onClick: () => void;
  disabled?: boolean;
  primary?: boolean;
  danger?: boolean;
  busy?: boolean;
  title?: string;
}) {
  const Icon = ICONS[props.icon];
  const cls = ["tool-btn", `icon-${props.icon}`, props.primary ? "primary" : "", props.danger ? "danger" : ""]
    .filter(Boolean)
    .join(" ");
  return (
    <button
      className={cls}
      onClick={props.onClick}
      disabled={props.disabled || props.busy}
      title={props.title ?? props.label}
      aria-label={props.label}
    >
      {props.busy ? (
        <span className="spinner" />
      ) : (
        <Icon size={16} />
      )}
    </button>
  );
}

export function ToolSep() {
  return <span className="tool-sep" />;
}

/** One field of the property strip: name, folder, notes. Read-only when there is no onChange. */
export type PropField = {
  label: string;
  value: string;
  onChange?: (value: string) => void;
  placeholder?: string;
  /** Suggestions for the field (the existing folders). */
  options?: string[];
  /** Description: a multi-line box. */
  multiline?: boolean;
  wide?: boolean;
};

/**
 * The one layout for every SQL page, catalog or saved, in the Vioros content style: the SQL on the
 * left (with its parameters and warnings above it), and the info panel on the right. The info panel
 * shows the name as a title and the properties (name, folder, notes) as the Vioros info table.
 * The divider between them drags, like the Vioros info panel.
 */
export function SqlPage(props: {
  crumbs: string[];
  title: string;
  actions: ReactNode;
  fields: PropField[];
  params?: ReactNode;
  banners?: ReactNode;
  sql: string;
  readOnly?: boolean;
  onSqlChange?: (sql: string) => void;
  onRun?: () => void;
  onCursor?: (line: number, column: number) => void;
  /** Shown over the page while it works, e.g. a running query. */
  overlay?: ReactNode;
  sqlEditor: (p: { value: string; readOnly: boolean; onChange?: (v: string) => void; onRun?: () => void; onCursor?: (l: number, c: number) => void }) => ReactNode;
}) {
  const listId = "page-options";
  const options = props.fields.find((f) => f.options)?.options ?? [];

  const [infoW, setInfoW] = useState(() => loadNumber("sql-info-w.v1", 300, 220, 800));
  function startInfoDrag(e: React.PointerEvent) {
    const start = infoW;
    let latest = start;
    beginDrag(
      e,
      "x",
      (d) => {
        latest = clamp(start + d, 220, 800);
        setInfoW(latest);
      },
      () => saveNumber("sql-info-w.v1", latest),
    );
  }

  return (
    <EditorPane crumbs={props.crumbs} title={props.title} actions={props.actions}>
      <div className="sql-split">
        <aside className="sql-info" style={{ width: infoW, minWidth: infoW, maxWidth: infoW }}>
          <InfoTable>
            {props.fields
              .filter((f) => !f.multiline)
              .map((f) => (
                <InfoLine key={f.label} label={f.label}>
                  <input
                    value={f.value}
                    readOnly={!f.onChange}
                    placeholder={f.placeholder}
                    list={f.options ? listId : undefined}
                    onChange={(e) => f.onChange?.(e.target.value)}
                  />
                </InfoLine>
              ))}
          </InfoTable>
          {/* Description: the Vioros description block, filling the rest of the panel */}
          {props.fields
            .filter((f) => f.multiline)
            .map((f) => (
              <div key={f.label} className="info-notes">
                <span className="info-key">{f.label}</span>
                <textarea
                  value={f.value}
                  readOnly={!f.onChange}
                  placeholder={f.placeholder}
                  onChange={(e) => f.onChange?.(e.target.value)}
                />
              </div>
            ))}
          {options.length > 0 && (
            <datalist id={listId}>
              {options.map((o) => (
                <option key={o} value={o} />
              ))}
            </datalist>
          )}
        </aside>

        <Splitter axis="x" onPointerDown={startInfoDrag} />

        <div className="sql-main">
          {props.params}
          {props.banners}
          <div className="sql-wrap">
            {props.sqlEditor({
              value: props.sql,
              readOnly: !!props.readOnly,
              onChange: props.onSqlChange,
              onRun: props.onRun,
              onCursor: props.onCursor,
            })}
          </div>
        </div>
      </div>
      {props.overlay}
    </EditorPane>
  );
}

/** The Vioros info table: a real table, rows with a grey label on the left and the value on the right. */
export function InfoTable(props: { children: ReactNode }) {
  return (
    <table className="info-table">
      <tbody>{props.children}</tbody>
    </table>
  );
}

/** One row of the info table. Editable values show as boxes; read-only values show as plain text. */
export function InfoLine(props: { label: ReactNode; required?: boolean; children: ReactNode }) {
  return (
    <tr>
      <th>
        {props.label}
        {props.required && <em> *</em>}
      </th>
      <td>{props.children}</td>
    </tr>
  );
}
