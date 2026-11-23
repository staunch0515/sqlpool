import { useCallback, useEffect, useState } from "react";
import { invoke } from "./api";
import { Banner, IconButton, SideSearch, Spinner } from "./components";
import { parentKeysOf, SideTree, TreeTools, useTreeCollapse, type TreeNode } from "./Tree";
import { formatTime, QueryHit, RunRecord, SavedGroup, SavedSql, SyncReport } from "./types";
import { FileCode, FilePlus, FolderPlus, Refresh, Star, Trash } from "./icons";

/** "qb.admin.attempts" -> "qb.admin". Ids with fewer than three segments go to "other". */
export function groupOf(queryId: string): string {
  const parts = queryId.split(".");
  return parts.length >= 3 ? `${parts[0]}.${parts[1]}` : "other";
}

/** Folders already indexed in this session. Re-reading 366 files on every switch made the app feel stuck. */
const indexedFolders = new Set<string>();

/** Catalog queries, grouped by the first two id segments. Read-only: nothing moves here. */
export function CatalogSide(props: {
  filter: string;
  onFilter: (value: string) => void;
  version: number;
  activeKey: string | null;
  onOpen: (hit: QueryHit) => void;
  settingsDir: string;
  onReindexed: (indexed: number, skipped: string[]) => void;
}) {
  const { filter, onFilter, version, activeKey, onOpen, settingsDir, onReindexed } = props;
  const tree = useTreeCollapse("side-catalog");
  const [loaded, setLoaded] = useState(false);
  const [busy, setBusy] = useState(false);
  const [hits, setHits] = useState<QueryHit[]>([]);
  const [error, setError] = useState<string | null>(null);

  const search = useCallback(async (text: string) => {
    try {
      setHits(await invoke<QueryHit[]>("search_queries", { text }));
      setError(null);
      setLoaded(true);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  async function reindex() {
    setBusy(true);
    try {
      const report = await invoke<SyncReport>("sync_catalog", { dir: settingsDir });
      onReindexed(report.indexed, report.skipped);
    } catch (e) {
      setError(String(e));
    }
    await search(filter);
    setBusy(false);
  }

  useEffect(() => {
    if (indexedFolders.has(settingsDir)) {
      search(filter).then(() => setLoaded(true));
    } else {
      indexedFolders.add(settingsDir);
      reindex();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [settingsDir]);

  useEffect(() => {
    const t = setTimeout(() => search(filter), 120);
    return () => clearTimeout(t);
  }, [filter, version, search]);

  const groups = [...new Set(hits.map((h) => groupOf(h.query_id)))];
  const nodes: TreeNode[] = [
    ...groups.map((g) => ({
      key: `g:${g}`,
      label: g,
      parent: null,
      kind: "group" as const,
      badge: <span className="count-badge">{hits.filter((h) => groupOf(h.query_id) === g).length}</span>,
    })),
    ...hits.map((h) => ({
      key: `catalog:${h.path}`,
      label: h.query_id,
      parent: `g:${groupOf(h.query_id)}`,
      kind: "item" as const,
      icon: <FileCode size={20}  />,
      tone: "catalog" as const,
      badge: h.favorite ? <Star size={12}  className="tree-star" /> : null,
    })),
  ];

  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search queries, tags, SQL" />
      <div className="side-toolbar">
        <IconButton icon={<Refresh size={15}  />} title="Reindex the catalog folder" onClick={reindex} busy={busy} />
        <span className="side-count">{hits.length} queries</span>
        <TreeTools onExpand={tree.expandAll} onCollapse={() => tree.collapseAll(parentKeysOf(nodes))} />
      </div>
      {error && <Banner kind="error">{error}</Banner>}
      <SideTree
        loading={!loaded}
        nodes={nodes}
        collapsed={tree.collapsed}
        onToggle={tree.toggle}
        activeKey={activeKey}
        onOpen={(key) => {
          const h = hits.find((x) => `catalog:${x.path}` === key);
          if (h) onOpen(h);
        }}
        filter={filter}
        empty={filter ? "No queries match this search." : "No queries. Check the catalog folder in Settings."}
      />
    </>
  );
}

/**
 * Saved statements in folders. Folders are rows of the tree too: drag a statement onto a folder to file it
 * there. Deleting a folder moves its statements to the top.
 */
export function SavedSide(props: {
  filter: string;
  onFilter: (value: string) => void;
  version: number;
  activeKey: string | null;
  onOpen: (item: SavedSql) => void;
  onNewStatement: (group: string) => void;
  onChanged: () => void;
}) {
  const { filter, onFilter, version, activeKey, onOpen, onNewStatement, onChanged } = props;
  const [list, setList] = useState<SavedSql[]>([]);
  const [groups, setGroups] = useState<SavedGroup[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const tree = useTreeCollapse("side-saved");

  useEffect(() => {
    (async () => {
      try {
        setList(await invoke<SavedSql[]>("list_saved_sql"));
        setGroups(await invoke<SavedGroup[]>("list_saved_groups"));
        setError(null);
        setLoaded(true);
      } catch (e) {
        setError(String(e));
      }
    })();
  }, [version]);

  async function createGroup() {
    const name = newName.trim();
    if (!name) return;
    try {
      await invoke("create_saved_group", { name });
      setNewName("");
      setAdding(false);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  async function deleteGroup(name: string) {
    if (pendingDelete !== name) {
      setPendingDelete(name);
      return;
    }
    try {
      await invoke("delete_saved_group", { name });
      setPendingDelete(null);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  async function moveStatement(s: SavedSql, group: string) {
    try {
      await invoke("save_saved_sql", { id: s.id, name: s.name, group, description: s.description, sql: s.sql });
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  // Folders only: a statement with no folder sits at the top of the tree, not in a bucket of its own.
  const names = [...new Set([...groups.map((g) => g.name), ...list.map((s) => s.group.trim())])].filter((n) => n !== "");
  const t = filter.trim().toLowerCase();
  const nodes: TreeNode[] = [
    ...names.map((name) => ({
      key: `g:${name}`,
      label: name,
      parent: null,
      kind: "group" as const,
      droppable: true,
      badge: <span className="count-badge">{list.filter((s) => s.group.trim() === name).length}</span>,
      actions: (
        <>
          <button className="icon-mini" title="New statement in this folder" onClick={() => onNewStatement(name)}>
            <FilePlus size={13}  />
          </button>
          <button
            className={pendingDelete === name ? "icon-mini danger-text" : "icon-mini"}
            title={pendingDelete === name ? "Click again to delete the folder" : "Delete folder (statements move to the top)"}
            onClick={() => deleteGroup(name)}
          >
            {pendingDelete === name ? "Confirm" : <Trash size={13}  />}
          </button>
        </>
      ),
    })),
    ...list
      .filter((s) => !t || [s.name, s.description, s.sql].some((x) => x.toLowerCase().includes(t)))
      .map((s) => ({
        key: `saved:${s.id}`,
        label: s.name,
        parent: s.group.trim() ? `g:${s.group.trim()}` : null,
        kind: "item" as const,
        icon: <FileCode size={20}  />,
        tone: "saved" as const,
      })),
  ];

  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search saved SQL" />
      <div className="side-toolbar">
        <IconButton icon={<FilePlus size={15}  />} title="New statement" onClick={() => onNewStatement("")} />
        <IconButton icon={<FolderPlus size={15}  />} title="New folder" onClick={() => setAdding(true)} />
        <span className="side-count">{list.length} saved</span>
        <TreeTools onExpand={tree.expandAll} onCollapse={() => tree.collapseAll(parentKeysOf(nodes))} />
      </div>
      {adding && (
        <div className="side-add">
          <input
            autoFocus
            value={newName}
            placeholder="Folder name"
            onChange={(e) => setNewName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") createGroup();
              if (e.key === "Escape") setAdding(false);
            }}
          />
          <button className="small" onClick={createGroup} disabled={!newName.trim()}>
            Add
          </button>
        </div>
      )}
      {error && <Banner kind="error">{error}</Banner>}
      <SideTree
        loading={!loaded}
        nodes={nodes}
        collapsed={tree.collapsed}
        onToggle={tree.toggle}
        activeKey={activeKey}
        onOpen={(key) => {
          const s = list.find((x) => `saved:${x.id}` === key);
          if (s) onOpen(s);
        }}
        onMove={(key, parent) => {
          const s = list.find((x) => `saved:${x.id}` === key);
          if (s) moveStatement(s, parent ? parent.slice(2) : "");
        }}
        dropToTop
        filter={filter}
        empty={filter ? "No statements match this search." : "No saved statements yet."}
      />
    </>
  );
}

/** Recent runs across all queries. Opening one goes to its query with the values it used. */
export function HistorySide(props: {
  filter: string;
  onFilter: (value: string) => void;
  version: number;
  onOpen: (run: RunRecord) => void;
}) {
  const { filter, onFilter, version, onOpen } = props;
  const [runs, setRuns] = useState<RunRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [tick, setTick] = useState(0);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    (async () => {
      try {
        setRuns(await invoke<RunRecord[]>("recent_runs", { limit: 200 }));
        setError(null);
        setLoaded(true);
      } catch (e) {
        setError(String(e));
      }
    })();
  }, [version, tick]);

  const shown = runs.filter((r) => {
    const t = filter.trim().toLowerCase();
    return !t || queryLabel(r.query_id).toLowerCase().includes(t) || r.params_json.toLowerCase().includes(t);
  });

  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search runs" />
      <div className="side-toolbar">
        <IconButton icon={<Refresh size={15}  />} title="Refresh" onClick={() => setTick((t) => t + 1)} />
        <span className="side-count">{runs.length} runs</span>
      </div>
      {error && <Banner kind="error">{error}</Banner>}
      {!loaded ? (
        <Spinner label="Loading runs…" />
      ) : (
        shown.length === 0 && <div className="side-empty">No runs yet.</div>
      )}
      <div className="tree">
        {shown.map((r) => (
          <button key={r.id} className="tree-item run-item" onClick={() => onOpen(r)}>
            <span className="tree-label">{queryLabel(r.query_id)}</span>
            <span className="meta small">
              {formatTime(r.ran_at)} · {r.ok ? `${r.rows ?? 0} rows` : "failed"}
            </span>
          </button>
        ))}
      </div>
    </>
  );
}

function queryLabel(queryId: string): string {
  return queryId.startsWith("saved:") ? `Saved statement #${queryId.slice(6)}` : queryId;
}
