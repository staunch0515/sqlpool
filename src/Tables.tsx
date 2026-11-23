import { useEffect, useState } from "react";
import { invoke } from "./api";
import { EditorPane, InfoLine, InfoTable, ToolButton, ToolSep } from "./EditorPane";
import { IconButton, SideSearch, Banner } from "./components";
import { Sitemap, Table } from "./icons";
import { SideTree, TreeNode, TreeTools, parentKeysOf, useTreeCollapse } from "./Tree";
import { QueryResult, Tab } from "./types";
import type { ToolIcon } from "./EditorPane";

/** A table of the mapping, with its columns (the fields of its entity). */
export type TableInfo = {
  entity: string;
  table: string;
  temporal: boolean;
  columns: { field: string; column: string }[];
};

export function tableKey(table: string): string {
  return `table:${table}`;
}

/** The group of a table: the part of its name before the first underscore (sec, sys, qb, …). */
export function groupOfTable(table: string): string {
  const i = table.indexOf("_");
  return i > 0 ? table.slice(0, i) : "other";
}

/**
 * The folders of a table. One level: the first part of its name (sys). Two levels: also the second part
 * (sys > account), so sys_account_deletion sits under sys › account. A name with two parts has no second level.
 */
export function folderPath(table: string, twoLevels: boolean): string[] {
  const parts = table.split("_");
  if (!twoLevels) return [groupOfTable(table)];
  if (parts.length >= 3) return [parts[0], parts[1]];
  return [groupOfTable(table)];
}

const TWO_LEVELS_KEY = "sqlpool.tables.twoLevels";

/**
 * The tables of the default data source's database, or null while unknown (no data source, or not loaded).
 * A table the mapping lists but the database does not have is shown as missing.
 */
export function useExistingTables(defaultDs: number | null): Set<string> | null {
  const [existing, setExisting] = useState<Set<string> | null>(null);
  useEffect(() => {
    if (defaultDs === null) {
      setExisting(null);
      return;
    }
    invoke<string[]>("existing_tables", { datasourceId: defaultDs })
      .then((names) => setExisting(new Set(names)))
      .catch(() => setExisting(null));
  }, [defaultDs]);
  return existing;
}

/** Tables grouped by their name prefix, like the catalog's folders. */
export function TablesSide(props: {
  defaultDs: number | null;
  mappingPath: string;
  filter: string;
  onFilter: (value: string) => void;
  activeKey: string | null;
  onOpen: (table: string) => void;
  version: number;
}) {
  const { defaultDs, mappingPath, filter, onFilter, activeKey, onOpen, version } = props;
  const existing = useExistingTables(defaultDs);
  const [list, setList] = useState<TableInfo[]>([]);
  const [error, setError] = useState<string | null>(null);
  const tree = useTreeCollapse("side-tables");
  const [twoLevels, setTwoLevels] = useState<boolean>(() => {
    try {
      return localStorage.getItem(TWO_LEVELS_KEY) === "1";
    } catch {
      return false;
    }
  });
  function toggleLevels() {
    setTwoLevels((v) => {
      try {
        localStorage.setItem(TWO_LEVELS_KEY, v ? "0" : "1");
      } catch {
        /* storage unavailable */
      }
      return !v;
    });
  }

  useEffect(() => {
    invoke<TableInfo[]>("list_tables", { mappingPath: mappingPath || null })
      .then((l) => {
        setList(l);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, [mappingPath, version]);

  const t = filter.trim().toLowerCase();
  const shown = list.filter((x) => !t || x.table.toLowerCase().includes(t) || x.entity.toLowerCase().includes(t));
  // Folder nodes for every level, keyed by their path, so a node under a folder has its parent in the list.
  const folderNodes = new Map<string, TreeNode>();
  for (const x of shown) {
    const path = folderPath(x.table, twoLevels);
    path.forEach((seg, i) => {
      const key = `g:${path.slice(0, i + 1).join("/")}`;
      if (!folderNodes.has(key)) {
        folderNodes.set(key, {
          key,
          label: seg,
          parent: i === 0 ? null : `g:${path.slice(0, i).join("/")}`,
          kind: "group" as const,
          badge: <span className="count-badge">{shown.filter((y) => folderPath(y.table, twoLevels).slice(0, i + 1).join("/") === path.slice(0, i + 1).join("/")).length}</span>,
        });
      }
    });
  }
  const nodes: TreeNode[] = [
    ...folderNodes.values(),
    ...shown.map((x) => {
      const path = folderPath(x.table, twoLevels);
      return {
        key: tableKey(x.table),
        label: x.table,
        missing: existing !== null && !existing.has(x.table),
        parent: `g:${path.join("/")}`,
        kind: "item" as const,
        icon: <Table size={14} />,
        badge: <span className="count-badge">{x.columns.length}</span>,
      };
    }),
  ];

  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search tables and entities" />
      <div className="side-toolbar">
        <span className="side-count">{list.length} tables</span>
        <IconButton
          icon={<Sitemap size={15} />}
          active={twoLevels}
          title={twoLevels ? "Grouped by two parts (sys › account). Click for one part." : "Group by two parts (sys › account)"}
          onClick={toggleLevels}
        />
        <TreeTools onExpand={tree.expandAll} onCollapse={() => tree.collapseAll(parentKeysOf(nodes))} />
      </div>
      {!mappingPath && <Banner kind="warn">Set the mapping file in Settings to list the tables.</Banner>}
      {error && <Banner kind="error">{error}</Banner>}
      <SideTree
        nodes={nodes}
        activeKey={activeKey}
        onOpen={(key) => {
          const x = list.find((y) => tableKey(y.table) === key);
          if (x) onOpen(x.table);
        }}
        filter={filter}
        empty={filter ? "No table matches." : "No tables."}
        collapsed={tree.collapsed}
        onToggle={tree.toggle}
      />
    </>
  );
}

/** A query a versioned table allows (from the backend). */
type VersionedDef = {
  id: string;
  title: string;
  description: string;
  params: { name: string; label: string; required: boolean }[];
};

const VERSIONED_ICON: Record<string, ToolIcon> = {
  current: "table",
  history: "history",
  asOf: "clock",
  changes: "diff",
  deleted: "trash",
};

/** A table in its tab: what it is, where it comes from, its columns, and the rows of the table. */
export function TableTab(props: {
  tab: Tab;
  mappingPath: string;
  defaultDs: number | null;
  onOutput: (key: string, out: { result: QueryResult | null; error: string | null; source?: string; datasourceId?: number | null; params?: string }) => void;
}) {
  const { tab, mappingPath, defaultDs, onOutput } = props;
  const name = tab.key.slice("table:".length);
  const [info, setInfo] = useState<TableInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [limited, setLimited] = useState(true);
  const [limit, setLimit] = useState("100");
  const [running, setRunning] = useState(false);
  const [versioned, setVersioned] = useState<VersionedDef[]>([]);
  const existing = useExistingTables(defaultDs);
  const missing = existing !== null && !existing.has(name);
  const [openQuery, setOpenQuery] = useState<VersionedDef | null>(null);
  const [queryValues, setQueryValues] = useState<Record<string, string>>({});
  // The columns ticked for the queries; a field that is not in the map is ticked.
  const [picked, setPicked] = useState<Record<string, boolean>>({});
  const isPicked = (field: string) => picked[field] !== false;
  /** The physical names of the ticked columns, in the table's order. */
  function pickedColumns(): string[] {
    return (info?.columns ?? []).filter((c) => isPicked(c.field)).map((c) => c.column);
  }
  const allPicked = !!info && info.columns.length > 0 && info.columns.every((c) => isPicked(c.field));
  function toggleAll() {
    const next = !allPicked;
    setPicked(Object.fromEntries((info?.columns ?? []).map((c) => [c.field, next])));
  }

  useEffect(() => {
    invoke<VersionedDef[]>("versioned_queries", { mappingPath: mappingPath || null, table: name })
      .then(setVersioned)
      .catch(() => setVersioned([]));
  }, [name, mappingPath]);

  /** Runs one of the versioned queries; its parameters go to the result, so Keep result stores them. */
  async function runVersioned(def: VersionedDef) {
    if (running) return;
    const params: Record<string, string> = {};
    for (const p of def.params) {
      const v = (queryValues[p.name] ?? "").trim();
      if (v) params[p.name] = v;
    }
    setRunning(true);
    try {
      const cols = pickedColumns();
      if (cols.length === 0) throw new Error("Tick at least one column.");
      const [result] = await invoke<[QueryResult, string]>("run_versioned_query", {
        args: { mappingPath: mappingPath || null, table: name, query: def.id, params, columns: cols, datasourceId: defaultDs },
      });
      onOutput(tab.key, {
        result,
        error: null,
        source: `table:${name}:${def.id}`,
        datasourceId: defaultDs,
        params: JSON.stringify({ query: def.title, ...params }),
      });
    } catch (e) {
      onOutput(tab.key, { result: null, error: String(e) });
    } finally {
      setRunning(false);
    }
  }

  function pressVersioned(def: VersionedDef) {
    if (def.params.length === 0) {
      setOpenQuery(null);
      runVersioned(def);
    } else {
      setOpenQuery(openQuery?.id === def.id ? null : def);
    }
  }

  async function selectRows() {
    if (running) return;
    setRunning(true);
    try {
      const cols = pickedColumns();
      if (cols.length === 0) throw new Error("Tick at least one column.");
      const result = await invoke<QueryResult>("select_table", {
        args: {
          table: name,
          limit: limited ? Number(limit) || 0 : null,
          columns: cols,
          datasourceId: defaultDs,
        },
      });
      onOutput(tab.key, { result, error: null, source: `table:${name}`, datasourceId: defaultDs });
    } catch (e) {
      onOutput(tab.key, { result: null, error: String(e) });
    } finally {
      setRunning(false);
    }
  }

  useEffect(() => {
    invoke<TableInfo[]>("list_tables", { mappingPath: mappingPath || null })
      .then((l) => {
        const x = l.find((y) => y.table === name) ?? null;
        setInfo(x);
        if (!x) setError(`The table ${name} is not in the mapping any more.`);
      })
      .catch((e) => setError(String(e)));
  }, [name, mappingPath]);

  const group = groupOfTable(name);
  return (
    <EditorPane
      crumbs={["Tables", group, name]}
      title={name}
      trailing={
        <>
          <label className="check table-limit">
            <input type="checkbox" checked={limited} onChange={(e) => setLimited(e.target.checked)} />
            <span>Limit rows</span>
          </label>
          <input
            className="table-limit-n"
            type="number"
            min={1}
            disabled={!limited}
            value={limit}
            onChange={(e) => setLimit(e.target.value)}
          />
        </>
      }
      actions={
        <>
          <ToolButton
            icon="play"
            label={running ? "Selecting…" : "Select the rows"}
            primary
            busy={running}
            disabled={running || !info}
            onClick={selectRows}
            title="Run SELECT on the default data source; the rows go to the results area"
          />
          <ToolSep />
          {versioned.map((def) => (
            <ToolButton
              key={def.id}
              icon={VERSIONED_ICON[def.id] ?? "table"}
              label={def.title}
              title={def.description}
              busy={running && openQuery?.id === def.id}
              disabled={running || missing}
              onClick={() => pressVersioned(def)}
            />
          ))}
          <ToolButton
            icon="checklist"
            label={allPicked ? "Untick all columns" : "Tick all columns"}
            disabled={!info}
            onClick={toggleAll}
            title="Which columns the queries return: tick the ones you want"
          />
          {versioned.length > 0 && <ToolSep />}
          <ToolButton
            icon="copy"
            label="Copy a SELECT"
            disabled={!info || missing}
            onClick={() => navigator.clipboard?.writeText(`SELECT *\nFROM ${name}\nLIMIT 100;`).catch(() => undefined)}
          />
          <ToolButton icon="copy" label="Copy the table name" onClick={() => navigator.clipboard?.writeText(name).catch(() => undefined)} />
          <ToolSep />
        </>
      }
    >
      <div className="sql-info env-page">
        {error && <Banner kind="error">{error}</Banner>}
        {missing && (
          <Banner kind="warn">
            {name} is in the mapping, but the default data source's database has no such table. Its queries are off until
            the database has it (the migration may not be applied there).
          </Banner>
        )}
        {openQuery && (
          <div className="versioned-params">
            <div className="analysis-label">{openQuery.title}</div>
            <div className="meta small">{openQuery.description}</div>
            <InfoTable>
              {openQuery.params.map((p) => (
                <InfoLine key={p.name} label={p.label}>
                  <input
                    value={queryValues[p.name] ?? ""}
                    placeholder={p.required ? "required" : "empty = now"}
                    onChange={(e) => setQueryValues({ ...queryValues, [p.name]: e.target.value })}
                  />
                </InfoLine>
              ))}
            </InfoTable>
            <div className="gen-actions">
              <ToolButton icon="play" label={running ? "Running…" : "Run"} primary busy={running} disabled={running} onClick={() => runVersioned(openQuery)} />
            </div>
          </div>
        )}
        {info && (
          <>
            <InfoTable>
              <InfoLine label="Entity">{info.entity}</InfoLine>
              <InfoLine label="Table">{info.table}</InfoLine>
              <InfoLine label="Versioned">{info.temporal ? "yes: read as the version in effect at a time (see the SQL template)" : "no: the rows are read directly"}</InfoLine>
              <InfoLine label="Columns">{info.columns.length}</InfoLine>
            </InfoTable>
            <div className="info-title">Columns</div>
            <table className="result-table">
              <thead>
                <tr>
                  <th className="col-pick">
                    <input type="checkbox" checked={allPicked} onChange={toggleAll} aria-label="Tick every column" />
                  </th>
                  <th>Field</th>
                  <th>Column</th>
                </tr>
              </thead>
              <tbody>
                {info.columns.map((c) => (
                  <tr key={c.field}>
                    <td className="col-pick">
                      <input
                        type="checkbox"
                        checked={isPicked(c.field)}
                        onChange={(e) => setPicked({ ...picked, [c.field]: e.target.checked })}
                        aria-label={`Include ${c.field}`}
                      />
                    </td>
                    <td>{c.field}</td>
                    <td className="mono">{c.column}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
      </div>
    </EditorPane>
  );
}
