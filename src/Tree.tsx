import { useState, type DragEvent, type ReactNode } from "react";
import { ChevronDown, ChevronRight, CollapseAll, ExpandAll, Folder, FolderOpen } from "./icons";

/**
 * One row of a side tree. A "group" is a folder: it holds rows, opens and closes, and can take drops.
 * An "item" is opened by clicking it; it takes drops only when `droppable` is set (environments).
 */
export type TreeNode = {
  key: string;
  label: string;
  /** Shown muted and italic: the item exists in the mapping but not where the app looks (e.g. the database). */
  missing?: boolean;
  /** Key of the parent row, or null for a top-level row. */
  parent: string | null;
  kind: "group" | "item";
  droppable?: boolean;
  /** Icon before the label of an item. Groups show a folder. */
  icon?: ReactNode;
  badge?: ReactNode;
  /** The colour family of an item's icon: catalog, saved or env. */
  tone?: "catalog" | "saved" | "env" | "ds";
  /** Small buttons at the right end of the row. They keep their place; they show when hovered or active. */
  actions?: ReactNode;
};

/** Keys of the rows that have children: what "collapse all" folds. */
export function parentKeysOf(nodes: TreeNode[]): string[] {
  const parents = new Set(nodes.map((n) => n.parent).filter((p): p is string => !!p));
  return nodes.filter((n) => parents.has(n.key)).map((n) => n.key);
}

/** Which rows are folded, remembered per storageKey. The panel shows the expand/collapse buttons (TreeTools). */
export function useTreeCollapse(storageKey: string) {
  const [collapsed, setCollapsed] = useState<string[]>(() => {
    try {
      return JSON.parse(localStorage.getItem(`${storageKey}.collapsed`) ?? "[]") as string[];
    } catch {
      return [];
    }
  });
  function save(next: string[]) {
    setCollapsed(next);
    try {
      localStorage.setItem(`${storageKey}.collapsed`, JSON.stringify(next));
    } catch {
      /* storage unavailable */
    }
  }
  return {
    collapsed,
    toggle: (key: string) => save(collapsed.includes(key) ? collapsed.filter((k) => k !== key) : [...collapsed, key]),
    expandAll: () => save([]),
    collapseAll: (keys: string[]) => save(keys),
  };
}

/** Expand all and collapse all, for the toolbar of a side panel. */
export function TreeTools(props: { onExpand: () => void; onCollapse: () => void }) {
  return (
    <>
      <button className="icon-sq" title="Expand all" aria-label="Expand all" onClick={props.onExpand}>
        <ExpandAll size={15}  />
      </button>
      <button className="icon-sq" title="Collapse all" aria-label="Collapse all" onClick={props.onCollapse}>
        <CollapseAll size={15}  />
      </button>
    </>
  );
}

/**
 * The one tree used by every side panel (catalog, saved SQL, environments). Its expand/collapse buttons are in
 * the panel's toolbar (TreeTools). Hover changes only the row's background; the actions keep their place.
 * Dragging an item onto a droppable row makes it a child of that row; onto the space below the list
 * it goes to the top (when `dropToTop` is set).
 */
export function SideTree(props: {
  nodes: TreeNode[];
  activeKey: string | null;
  onOpen: (key: string) => void;
  onMove?: (key: string, parent: string | null) => void;
  dropToTop?: boolean;
  filter: string;
  empty: string;
  loading?: boolean;
  /** Folded rows, and the toggle, from useTreeCollapse. */
  collapsed: string[];
  onToggle: (key: string) => void;
}) {
  const { nodes, activeKey, onOpen, onMove, dropToTop, filter, empty, loading, collapsed, onToggle } = props;
  const byKey = new Map(nodes.map((n) => [n.key, n]));
  const children = new Map<string, TreeNode[]>();
  const roots: TreeNode[] = [];
  for (const n of nodes) {
    if (n.parent && n.parent !== n.key && byKey.has(n.parent)) {
      children.set(n.parent, [...(children.get(n.parent) ?? []), n]);
    } else {
      roots.push(n);
    }
  }
  const byLabel = (a: TreeNode, b: TreeNode) => a.label.localeCompare(b.label, undefined, { sensitivity: "base", numeric: true });

  // A flat list (no folders, no nested rows) has no chevron column, so its icons sit at the left edge.
  const hierarchy = nodes.some((n) => n.kind === "group" || children.has(n.key));

  const [dragKey, setDragKey] = useState<string | null>(null);
  const [overKey, setOverKey] = useState<string | "top" | null>(null);

  /** True when `key` is `ancestor` or sits below it. */
  function isWithin(key: string, ancestor: string): boolean {
    let cur: string | null = key;
    const seen = new Set<string>();
    while (cur && !seen.has(cur)) {
      if (cur === ancestor) return true;
      seen.add(cur);
      cur = byKey.get(cur)?.parent ?? null;
    }
    return false;
  }

  /** A row may go under `target` (null: the top) unless that is where it already is, or under itself. */
  function canDrop(dragged: string, target: string | null): boolean {
    const node = byKey.get(dragged);
    if (!node || !onMove) return false;
    if (target === null) return !!dropToTop && node.parent !== null;
    const t = byKey.get(target);
    if (!t || !(t.kind === "group" || t.droppable)) return false;
    if (target === node.parent) return false;
    return !isWithin(target, dragged);
  }

  const dropHandlers = (key: string | null) => ({
    onDragOver: (e: DragEvent) => {
      if (dragKey && canDrop(dragKey, key)) {
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        setOverKey(key ?? "top");
      }
    },
    onDragLeave: () => setOverKey(null),
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      if (dragKey && canDrop(dragKey, key)) onMove?.(dragKey, key);
      setDragKey(null);
      setOverKey(null);
    },
  });

  // Visible rows, in tree order. A search lists the matching items flat.
  const rows: { node: TreeNode; depth: number; hasKids: boolean; open: boolean }[] = [];
  const searching = filter.trim() !== "";
  if (searching) {
    const t = filter.trim().toLowerCase();
    for (const n of [...nodes].filter((x) => x.kind === "item").sort(byLabel)) {
      if (n.label.toLowerCase().includes(t)) rows.push({ node: n, depth: 0, hasKids: false, open: false });
    }
  } else {
    const walk = (list: TreeNode[], depth: number) => {
      for (const n of [...list].sort((a, b) => (a.kind === b.kind ? byLabel(a, b) : a.kind === "group" ? -1 : 1))) {
        const kids = children.get(n.key) ?? [];
        const open = !collapsed.includes(n.key);
        rows.push({ node: n, depth, hasKids: kids.length > 0, open });
        if (kids.length && open) walk(kids, depth + 1);
      }
    };
    walk(roots, 0);
  }

  return loading && nodes.length === 0 ? (
    <div className="side-empty">Loading…</div>
  ) : (
    <div className="tree node-tree">
      {rows.length === 0 && <div className="side-empty">{empty}</div>}
      {rows.map(({ node, depth, hasKids, open }) => {
        const isGroup = node.kind === "group";
        const cls = [
          "tree-row",
          node.key === activeKey ? "active" : "",
          overKey === node.key ? "drop-over" : "",
          isGroup ? "is-group" : "",
          node.tone ? `tone-${node.tone}` : "",
          node.missing ? "missing" : "",
        ]
          .filter(Boolean)
          .join(" ");
        return (
          <div
            key={node.key}
            className={cls}
            style={{ paddingLeft: 8 + depth * 14 }}
            draggable={!!onMove && !isGroup}
            onDragStart={(e) => {
              e.dataTransfer.setData("text/plain", node.key);
              e.dataTransfer.effectAllowed = "move";
              setDragKey(node.key);
            }}
            onDragEnd={() => {
              setDragKey(null);
              setOverKey(null);
            }}
            {...dropHandlers(node.key)}
            onClick={() => (isGroup ? onToggle(node.key) : onOpen(node.key))}
          >
            {hierarchy && (
              <span className="tree-chev">
                {(isGroup || hasKids) && !searching ? (
                  open ? <ChevronDown size={18}  /> : <ChevronRight size={18}  />
                ) : null}
              </span>
            )}
            <span className="tree-icon">
              {isGroup ? (open ? <FolderOpen size={20}  /> : <Folder size={20}  />) : node.icon}
            </span>
            <span className="tree-label">{node.label}</span>
            {node.badge}
            <span className="tree-row-actions" onClick={(e) => e.stopPropagation()}>
              {node.actions}
            </span>
          </div>
        );
      })}
      {onMove && dropToTop && dragKey && (
        <div className={overKey === "top" ? "tree-drop-top drop-over" : "tree-drop-top"} {...dropHandlers(null)}>
          Drop here to move to the top
        </div>
      )}
    </div>
  );
}
