import { useEffect, useState } from "react";
import { invoke } from "./api";
import { Banner, SideSearch } from "./components";
import { Download, Sparkles, Trash } from "./icons";
import { SideTree } from "./Tree";
import { SavedResult } from "./types";

/**
 * The results kept with "Keep result": open one into the active tab's bottom panel, or delete it
 * (the second click on the trash icon confirms).
 */
export function SavedResultsSide(props: {
  filter: string;
  onFilter: (value: string) => void;
  version: number;
  onOpen: (id: number, name: string) => void;
  onOpenAnalysis: (id: number, name: string) => void;
  activeKey: string | null;
}) {
  const { filter, onFilter, version, onOpen, onOpenAnalysis, activeKey } = props;
  const [list, setList] = useState<SavedResult[]>([]);
  const [analyses, setAnalyses] = useState<{ id: number; name: string; result_id: number | null }[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<number | null>(null);
  const [reload, setReload] = useState(0);

  useEffect(() => {
    invoke<SavedResult[]>("list_saved_results")
      .then((l) => {
        setList(l);
        setError(null);
      })
      .catch((e) => setError(String(e)));
    invoke<{ id: number; name: string; result_id: number | null }[]>("list_saved_analyses")
      .then(setAnalyses)
      .catch(() => setAnalyses([]));
  }, [version, reload]);

  async function remove(id: number) {
    if (pending !== id) {
      setPending(id);
      return;
    }
    try {
      await invoke("delete_saved_result", { id });
      setPending(null);
      setReload((n) => n + 1);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search saved results" />
      <div className="side-toolbar">
        <span className="side-count">
          {list.length} results · {analyses.length} analyses
        </span>
      </div>
      {error && <Banner kind="error">{error}</Banner>}
      <SideTree
        nodes={[
          ...analyses.map((a) => ({
            key: `analysis:saved-${a.id}`,
            label: a.name,
            // An analysis hangs under the result it came from; one without a result stays at the top.
            parent: a.result_id !== null && list.some((r) => r.id === a.result_id) ? `result:${a.result_id}` : null,
            kind: "item" as const,
            icon: <Sparkles size={14} />,
            badge: <span className="count-badge">AI</span>,
          })),
          ...list.map((r) => ({
          key: `result:${r.id}`,
          label: r.name,
          parent: null,
          kind: "item" as const,
          icon: <Download size={14} />,
          badge: <span className="count-badge">{r.row_count}</span>,
          actions: (
            <button
              className={pending === r.id ? "icon-mini danger-text" : "icon-mini"}
              title={pending === r.id ? "Click again to delete this result" : "Delete this result"}
              onClick={() => remove(r.id)}
            >
              {pending === r.id ? "Confirm" : <Trash size={13} />}
            </button>
          ),
        })),
        ]}
        activeKey={activeKey}
        onOpen={(key) => {
          if (key.startsWith("analysis:saved-")) {
            const id = Number(key.slice("analysis:saved-".length));
            const a = analyses.find((x) => x.id === id);
            if (a) onOpenAnalysis(id, a.name);
            return;
          }
          const id = Number(key.slice("result:".length));
          const row = list.find((r) => r.id === id);
          if (row) onOpen(id, row.name);
        }}
        filter={filter}
        empty={list.length === 0 ? "No results kept yet. Use Keep result under the results." : "No match."}
        collapsed={[]}
        onToggle={() => undefined}
      />
    </>
  );
}
