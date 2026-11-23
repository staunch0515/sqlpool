import { useEffect, useState } from "react";
import { invoke } from "./api";
import { EditorPane, ToolButton, ToolSep } from "./EditorPane";
import { exportResult } from "./exportResult";
import { Datasource, formatTime, QueryResult, SavedResult, Tab } from "./types";

/** The key for a kept result's tab. */
export function resultKey(id: number): string {
  return `result:${id}`;
}

/**
 * A kept result in its own tab: its toolbar (export, copy the SQL, delete), the data source it ran on,
 * the SQL that was sent, and the rows.
 */
export function SavedResultTab(props: {
  tab: Tab;
  datasources: Datasource[];
  exportDir: string;
  onExportDir: (dir: string) => void;
  onChanged: () => void;
  onDeleted: (key: string) => void;
  /** The rows go to the bottom panel, for this tab, like a query's rows do. */
  onOutput: (key: string, out: { result: QueryResult | null; error: string | null; source?: string; datasourceId?: number | null }) => void;
}) {
  const { tab, datasources, exportDir, onExportDir, onChanged, onDeleted, onOutput } = props;
  const id = Number(tab.key.slice("result:".length));
  const [meta, setMeta] = useState<SavedResult | null>(null);
  const [result, setResult] = useState<QueryResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  // A message fades out by itself; it floats over the tab, so nothing below it moves.
  useEffect(() => {
    if (!error && !notice) return;
    const t = setTimeout(() => {
      setError(null);
      setNotice(null);
    }, 4000);
    return () => clearTimeout(t);
  }, [error, notice]);

  useEffect(() => {
    (async () => {
      try {
        const all = await invoke<SavedResult[]>("list_saved_results");
        const m = all.find((r) => r.id === id) ?? null;
        const r = await invoke<QueryResult>("read_saved_result", { id });
        setMeta(m);
        setResult(r);
        setError(null);
        onOutput(tab.key, { result: r, error: null, source: m?.source ?? "", datasourceId: m?.datasource_id ?? null });
      } catch (e) {
        setError(String(e));
      }
    })();
  }, [id]);

  const dsName = meta?.datasource_id != null ? datasources.find((d) => d.id === meta.datasource_id)?.name ?? "removed data source" : "—";

  async function exportCsv() {
    if (!result) return;
    try {
      const path = await exportResult(result, tab.title, exportDir, onExportDir);
      if (path) setNotice(`Exported to ${path}`);
    } catch (e) {
      setError(String(e));
    }
  }

  async function remove() {
    try {
      await invoke("delete_saved_result", { id });
      onChanged();
      onDeleted(tab.key);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <EditorPane
      crumbs={["Saved results", tab.title]}
      title={tab.title}
      actions={
        <>
          <ToolButton icon="download" label="Export CSV" primary disabled={!result} onClick={exportCsv} />
          <ToolButton
            icon="copy"
            label="Copy SQL"
            disabled={!result}
            onClick={() => result && navigator.clipboard?.writeText(result.executed_sql).catch(() => undefined)}
          />
          <ToolSep />
          {confirmDelete ? (
            <ToolButton icon="x" label="Confirm delete" danger onClick={remove} />
          ) : (
            <ToolButton icon="trash" label="Delete this result" onClick={() => setConfirmDelete(true)} />
          )}
        </>
      }
    >
      <div className="result-page">
        {(error || notice) && (
          <div
            className={error ? "panel-toast error" : "panel-toast"}
            role="status"
            onClick={() => {
              setError(null);
              setNotice(null);
            }}
            title="Click to dismiss"
          >
            {error ?? notice}
          </div>
        )}
        <div className="result-facts">
          <span>
            <b>Data source</b> {dsName}
          </span>
          <span>
            <b>Source</b> {meta?.source || "—"}
          </span>
          <span>
            <b>Rows</b> {meta?.row_count ?? result?.rows.length ?? 0}
            {meta?.truncated ? " (truncated)" : ""}
          </span>
          <span>
            <b>Kept</b> {formatTime(meta?.created_at ?? null)}
          </span>
          {meta?.params && (
            <span>
              <b>Parameters</b> {meta.params}
            </span>
          )}
        </div>
        {result && <pre className="sent-sql result-sql">{result.executed_sql}</pre>}
      </div>
    </EditorPane>
  );
}
