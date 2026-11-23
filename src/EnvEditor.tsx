import { useEffect, useRef, useState } from "react";
import { invoke } from "./api";
import { IconButton, SideSearch, Toast } from "./components";
import { FilePlus, InfoFilled, Layers, Plus } from "./icons";
import { parentKeysOf, SideTree, TreeNode, TreeTools, useTreeCollapse } from "./Tree";
import { EditorPane, ToolButton, ToolSep } from "./EditorPane";
import { DataSourceSelect } from "./DataSources";
import { Datasource, Environment, effectiveValues, envTree, Tab } from "./types";

/** The key for an environment's tab. */
export function envKey(name: string): string {
  return `env:${name}`;
}

/** Side tree of environments. Drag one onto another to make it a child; onto the space below to move it to the top. */
export function EnvSide(props: {
  envs: Environment[];
  filter: string;
  onFilter: (value: string) => void;
  activeKey: string | null;
  activeEnv: string;
  onOpen: (name: string) => void;
  onNew: (parent: string | null) => void;
  onMove: (name: string, parent: string | null) => void;
}) {
  const { envs, filter, onFilter, activeKey, activeEnv, onOpen, onNew, onMove } = props;
  const tree = useTreeCollapse("side-env");
  const nodes: TreeNode[] = envs.map((e) => ({
      key: envKey(e.name),
      label: e.name,
      parent: e.parent ? envKey(e.parent) : null,
      kind: "item" as const,
      droppable: true,
      icon: <Layers size={20}  />,
      tone: "env" as const,
      badge: (
        <>
          {e.name === activeEnv && <span className="env-in-use">in use</span>}
          <span className="count-badge">{Object.keys(e.values).length}</span>
        </>
      ),
      actions: (
        <button className="icon-mini" title="New child environment" onClick={() => onNew(e.name)}>
          <Plus size={13}  />
        </button>
      ),
  }));

  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search environments" />
      <div className="side-toolbar">
        <IconButton icon={<FilePlus size={15}  />} title="New environment" onClick={() => onNew(null)} />
        <span className="side-count">{envs.length} environments</span>
        <TreeTools onExpand={tree.expandAll} onCollapse={() => tree.collapseAll(parentKeysOf(nodes))} />
      </div>
      <SideTree
        filter={filter}
        nodes={nodes}
        collapsed={tree.collapsed}
        onToggle={tree.toggle}
        activeKey={activeKey}
        onOpen={(key) => {
          const e = envs.find((x) => envKey(x.name) === key);
          if (e) onOpen(e.name);
        }}
        onMove={(key, parent) => {
          const e = envs.find((x) => envKey(x.name) === key);
          if (e) onMove(e.name, parent ? parent.slice("env:".length) : null);
        }}
        empty={envs.length === 0 ? "No environments yet." : "No match."}
      />
    </>
  );
}

type Row = { key: string; value: string };

const BLANK: Row = { key: "", value: "" };

function rowsOf(env: Environment | null): Row[] {
  const rows = env ? Object.entries(env.values).map(([key, value]) => ({ key, value })) : [];
  return [...rows, { ...BLANK }];
}

/**
 * Editor tab for one environment, laid out like a saved statement: breadcrumb, toolbar, then
 * the name, the parent (picked from the tree), the description, and the parameter values.
 * A new row appears by itself when the last one is used. Empty placeholders show what the
 * parent gives.
 */
export function EnvEditor(props: {
  tab: Tab;
  envs: Environment[];
  activeEnv: string;
  datasources: Datasource[];
  onUse: (name: string) => void;
  onChanged: () => void;
  onRenamed: (oldKey: string, t: Tab) => void;
  onDeleted: (key: string) => void;
}) {
  const { tab, envs, activeEnv, datasources, onUse, onChanged, onRenamed, onDeleted } = props;
  const original = envs.find((e) => envKey(e.name) === tab.key) ?? null;
  const [datasourceId, setDatasourceId] = useState<number | null>(original?.datasource_id ?? null);
  const [name, setName] = useState(original?.name ?? "");
  // The parent is changed in the side tree, never here: a new one starts under the node it was created from.
  const parent = original ? original.parent : tab.group || null;
  const [description, setDescription] = useState(original?.description ?? "");
  const [rows, setRows] = useState<Row[]>(() => rowsOf(original));
  const [loadedFor, setLoadedFor] = useState<string | null>(original ? tab.key : null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  // The first time this tab's environment is known, fill the form from it.
  if (original && loadedFor !== tab.key) {
    setLoadedFor(tab.key);
    setName(original.name);
    setDescription(original.description);
    setDatasourceId(original.datasource_id);
    setRows(rowsOf(original));
  }

  const title = name.trim() || "New environment";
  const isNew = original === null;
  const inherited = parent ? effectiveValues(envs, parent) : {};

  async function save() {
    setError(null);
    setNotice(null);
    const trimmed = name.trim();
    if (!trimmed) return setError("Give the environment a name.");
    const values: Record<string, string> = {};
    for (const r of rows) {
      const k = r.key.trim();
      if (!k) continue;
      if (k in values) return setError(`Parameter ${k} is listed twice.`);
      values[k] = r.value;
    }
    try {
      if (original && original.name !== trimmed) {
        await invoke("rename_environment", { old: original.name, new: trimmed });
        if (activeEnv === original.name) onUse(trimmed);
      }
      await invoke("save_environment", { name: trimmed, parent, description, values, datasourceId });
      onChanged();
      if (envKey(trimmed) !== tab.key) {
        onRenamed(tab.key, { key: envKey(trimmed), kind: "env", title: trimmed, queryId: "" });
      } else {
        setNotice("Saved.");
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function remove() {
    if (!original) return;
    try {
      await invoke("delete_environment", { name: original.name });
      if (activeEnv === original.name) onUse("");
      onChanged();
      onDeleted(tab.key);
    } catch (e) {
      setError(String(e));
    }
  }

  const [showAll, setShowAll] = useState(false);
  const ownValues: Record<string, string> = {};
  for (const r of rows) if (r.key.trim() && r.value !== "") ownValues[r.key.trim()] = r.value;
  const family = familyOf(envs, original?.name ?? title, parent, ownValues);
  const allRows = showAll ? allParameters(envs, parent, rows) : [];

  /** Sets this environment's own value for a parameter (used when a parent's value is changed here). */
  function setOwnValue(key: string, value: string) {
    setRows((rs) => {
      const idx = rs.findIndex((r) => r.key.trim() === key);
      if (idx >= 0) return rs.map((r, j) => (j === idx ? { ...r, value } : r));
      const filled = rs.filter((r) => r.key.trim() !== "");
      return [...filled, { key, value }, { ...BLANK }];
    });
  }

  /** Edits a row; typing in the last row adds a new empty one below it. */
  function setRow(i: number, patch: Partial<Row>) {
    setRows((rs) => {
      const next = rs.map((r, j) => (j === i ? { ...r, ...patch } : r));
      const last = next[next.length - 1];
      return last.key || last.value ? [...next, { ...BLANK }] : next;
    });
  }

  return (
    <EditorPane
      crumbs={["Environments", ...ancestorNames(envs, parent), title]}
      title={title}
      actions={
        <>
          <ToolButton icon="save" label="Save" primary onClick={save} />
          <ToolButton
            icon="tree"
            label={showAll ? "Own parameters only" : "Show all parameters"}
            title="Show the parents' parameters too. Change one to make it this environment's own."
            onClick={() => setShowAll((v) => !v)}
          />
          <ToolButton
            icon={activeEnv && activeEnv === original?.name ? "starFilled" : "star"}
            label={activeEnv && activeEnv === original?.name ? "In use" : "Use in forms"}
            disabled={isNew}
            title="Forms fill their parameters from the environment in use"
            onClick={() => original && onUse(activeEnv === original.name ? "" : original.name)}
          />
          <ToolSep />
          {!isNew &&
            (confirmDelete ? (
              <ToolButton icon="x" label="Confirm delete" danger onClick={remove} />
            ) : (
              <ToolButton icon="trash" label="Delete" onClick={() => setConfirmDelete(true)} />
            ))}
        </>
      }
    >
      <div className="sql-info env-page">
        <Toast kind={error ? "error" : "ok"} text={error ?? notice} onDismiss={() => { setError(null); setNotice(null); }} />
        <table className="info-table env-table">
          <colgroup>
            <col className="env-col-label" />
            <col />
          </colgroup>
          <tbody>
            <tr>
              <th>Name</th>
              <td>
                <input value={name} placeholder="e.g. Staging" onChange={(e) => setName(e.target.value)} />
              </td>
            </tr>
            <tr>
              <th>Data source</th>
              <td>
                <DataSourceSelect value={datasourceId} datasources={datasources} onChange={setDatasourceId} />
              </td>
            </tr>
            <tr>
              <th>Description</th>
              <td>
                <textarea
                  className="env-description"
                  rows={3}
                  value={description}
                  placeholder="What this environment is for"
                  onChange={(e) => setDescription(e.target.value)}
                />
              </td>
            </tr>
            <tr className="env-section">
              <th colSpan={3}>{showAll ? "All parameters, with the parent's values" : "This environment's own parameters"}</th>
            </tr>
            {showAll
              ? allRows.map((r) => (
                  <tr key={r.key}>
                    <th>
                      <span className="env-key-label">
                        <span className="env-key-text">{r.key}</span>
                        {r.origin !== "this environment" && <TreeHint family={family} name={r.key} />}
                      </span>
                    </th>
                    <td>
                      <input value={r.value} placeholder="empty = NULL" onChange={(e) => setOwnValue(r.key, e.target.value)} />
                    </td>
                  </tr>
                ))
              : rows.map((r, i) => {
                  const key = r.key.trim();
                  const fromParent = key && key in inherited ? inherited[key] : undefined;
                  return (
                    <tr key={i}>
                      <th>
                        <input className="env-key" placeholder="parameter" value={r.key} onChange={(e) => setRow(i, { key: e.target.value })} />
                      </th>
                      <td>
                        <input
                          placeholder={fromParent !== undefined ? `from parent: ${fromParent}` : "empty = NULL"}
                          value={r.value}
                          onChange={(e) => setRow(i, { value: e.target.value })}
                        />
                      </td>
                    </tr>
                  );
                })}
            {showAll && allRows.length === 0 && (
              <tr>
                <th>—</th>
                <td>
                  <span className="meta small">No parameters here or in the parents.</span>
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </EditorPane>
  );
}

/**
 * Every parameter an environment gets: the parents' values (the nearest parent wins) and its own.
 * Each row says where its value comes from.
 */
function allParameters(envs: Environment[], parent: string | null, rows: Row[]): { key: string; value: string; origin: string }[] {
  const byName = new Map(envs.map((e) => [e.name, e]));
  const chain: Environment[] = [];
  const seen = new Set<string>();
  let cur = parent ? byName.get(parent) : undefined;
  while (cur && !seen.has(cur.name)) {
    seen.add(cur.name);
    chain.unshift(cur);
    cur = cur.parent ? byName.get(cur.parent) : undefined;
  }
  const inherited = new Map<string, { value: string; from: string }>();
  for (const e of chain) for (const [k, v] of Object.entries(e.values)) inherited.set(k, { value: v, from: e.name });

  const own = new Map<string, string>();
  for (const r of rows) {
    const k = r.key.trim();
    if (k && r.value !== "") own.set(k, r.value);
  }
  const keys = [...new Set([...own.keys(), ...inherited.keys()])];
  return keys.map((key) => {
    const ownValue = own.get(key);
    if (ownValue !== undefined) return { key, value: ownValue, origin: "this environment" };
    const inh = inherited.get(key)!;
    return { key, value: inh.value, origin: `from ${inh.from}` };
  });
}

/**
 * The tree this environment belongs to (its root and everything below it), with this
 * environment's values as they are in the form, not as saved.
 */
function familyOf(
  envs: Environment[],
  self: string,
  parent: string | null,
  ownValues: Record<string, string>,
): { name: string; parent: string | null; values: Record<string, string>; isSelf: boolean; depth: number }[] {
  const list: Environment[] = [
    ...envs.filter((e) => e.name !== self),
    { name: self, parent, datasource_id: null, description: "", values: ownValues },
  ];
  const byName = new Map(list.map((e) => [e.name, e]));
  // the chain up from this environment: the parents it inherits from
  const chain = new Set<string>();
  let cur: Environment | undefined = byName.get(self);
  while (cur && !chain.has(cur.name)) {
    chain.add(cur.name);
    cur = cur.parent ? byName.get(cur.parent) : undefined;
  }
  const root = [...chain].pop() ?? self;
  const tree = envTree(list);
  const start = tree.findIndex((t) => t.env.name === root);
  if (start < 0) return [];
  const out: { name: string; parent: string | null; values: Record<string, string>; isSelf: boolean; depth: number }[] = [];
  for (let i = start; i < tree.length && (i === start || tree[i].depth > 0); i++) {
    const e = tree[i].env;
    out.push({ name: e.name, parent: e.parent, values: e.values, isSelf: e.name === self, depth: tree[i].depth });
  }
  return out;
}

/** The small icon after a parameter name: on hover, every value this parameter has in the tree. */
function TreeHint(props: {
  family: { name: string; values: Record<string, string>; isSelf: boolean; depth: number; parent: string | null }[];
  name: string;
}) {
  const key = props.name.trim();
  const hits = key ? props.family.filter((f) => key in f.values) : [];
  return (
    <span className="env-hint" tabIndex={0} aria-label="Values in the tree">
      <InfoFilled size={16} className="info-hint-icon" />
      <span className="env-hint-pop" role="tooltip">
        <span className="env-hint-title">{key || "parameter"} in this tree</span>
        {hits.length === 0 ? (
          <span className="env-hint-empty">not set anywhere in the tree</span>
        ) : (
          <table className="env-hint-table">
            <thead>
              <tr>
                <th>Environment</th>
                <th>Value</th>
              </tr>
            </thead>
            <tbody>
              {hits.map((h) => (
                <tr key={h.name} className={h.isSelf ? "this" : undefined}>
                  <td style={{ paddingLeft: 8 + h.depth * 14 }}>
                    {h.depth > 0 && <span className="parent-branch">└</span>}
                    {h.name}
                    {h.isSelf && <span className="env-hint-this"> (this)</span>}
                  </td>
                  <td className="env-hint-value">{h.values[key] === "" ? "NULL" : h.values[key]}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </span>
    </span>
  );
}

/** The names from the root down to the parent, for the breadcrumb. */
function ancestorNames(envs: Environment[], parent: string | null): string[] {
  const byName = new Map(envs.map((e) => [e.name, e]));
  const names: string[] = [];
  const seen = new Set<string>();
  let cur = parent ? byName.get(parent) : undefined;
  while (cur && !seen.has(cur.name)) {
    seen.add(cur.name);
    names.unshift(cur.name);
    cur = cur.parent ? byName.get(cur.parent) : undefined;
  }
  return names;
}

/**
 * The environment in use, picked in the top bar. A square button, as the other buttons; the menu is the
 * tree of environments, indented by depth, and opens right under the button.
 */
export function EnvPicker(props: { envs: Environment[]; value: string; onChange: (name: string) => void }) {
  const { envs, value, onChange } = props;
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

  // The full path of the environment in use: its ancestors from the root down, then itself.
  const label = value ? [...ancestorNames(envs, envs.find((e) => e.name === value)?.parent ?? null), value].join(" › ") : "None";
  return (
    <div className="env-picker">
      <span className="meta">Current Environment:</span>
      <div className="env-picker-anchor" ref={box}>
      <button type="button" className="env-picker-button" aria-haspopup="listbox" aria-expanded={open} title="Forms fill their parameters from this environment first" onClick={() => setOpen((o) => !o)}>
        {label}
      </button>
      {open && (
        <div className="env-picker-menu" role="listbox">
          <div
            role="option"
            aria-selected={value === ""}
            className={value === "" ? "env-picker-item active" : "env-picker-item"}
            onClick={() => {
              onChange("");
              setOpen(false);
            }}
          >
            None
          </div>
          {envTree(envs).map(({ env, depth }) => (
            <div
              key={env.name}
              role="option"
              aria-selected={value === env.name}
              className={value === env.name ? "env-picker-item active" : "env-picker-item"}
              style={{ paddingLeft: 12 + depth * 16 }}
              onClick={() => {
                onChange(env.name);
                setOpen(false);
              }}
            >
              {depth > 0 && <span className="env-picker-branch">└</span>}
              {env.name}
            </div>
          ))}
        </div>
      )}
      </div>
    </div>
  );
}
