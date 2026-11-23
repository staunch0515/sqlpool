import { useEffect, useRef, useState } from "react";
import { invoke } from "./api";
import { Banner, ResultTable, Spinner } from "./components";
import { Copy, Download, Save, Sparkles } from "./icons";
import { formatTime, Output, parseParams, QueryResult, RunRecord, Tab } from "./types";
import { exportResult } from "./exportResult";

/**
 * Bottom panel (VS Code): the output of the active tab, the SQL that was sent for it, and its run history.
 * "Reuse values" on a run fills the active tab's form with the values that run used. A result can be
 * exported as CSV, or kept (saved on disk, listed under Saved results).
 */
export function BottomPanel(props: {
  /** Something is running (a query, a save, a reindex). Shows a timer in the results. */
  busy: boolean;
  tab: Tab | null;
  output: Output | undefined;
  version: number;
  onReuse: (run: RunRecord) => void;
  /** A result was kept: the saved results list should reload. */
  onKept: () => void;
  /** The export folder from Settings. Empty: the first export asks for one, and it is remembered. */
  exportDir: string;
  onExportDir: (dir: string) => void;
  /** Opens the AI analysis of this result in its own tab. */
  onAnalyze: (result: QueryResult, tab: Tab, datasourceId: number | null) => void;
}) {
  const { tab, output, version, onReuse, onKept, exportDir, onExportDir, onAnalyze } = props;
  const [notice, setNotice] = useState<string | null>(null);

  const [view, setView] = useState<"results" | "console" | "sql" | "runs">("results");
  const [runs, setRuns] = useState<RunRecord[]>([]);
  const [loadingRuns, setLoadingRuns] = useState(false);

  useEffect(() => {
    if (!tab || !tab.queryId) {
      setRuns([]);
      return;
    }
    setLoadingRuns(true);
    (async () => {
      try {
        setRuns(await invoke<RunRecord[]>("query_history", { queryId: tab.queryId, limit: 50 }));
      } catch {
        setRuns([]);
      } finally {
        setLoadingRuns(false);
      }
    })();
  }, [tab?.key, version]);

  useEffect(() => setNotice(null), [tab?.key]);

  // A message fades out by itself, so nothing stays on screen.
  useEffect(() => {
    if (!notice) return;
    const t = setTimeout(() => setNotice(null), 4000);
    return () => clearTimeout(t);
  }, [notice]);

  const result = output?.result ?? null;
  const title = tab?.title ?? "result";

  async function exportCsv() {
    if (!result) return;
    try {
      const path = await exportResult(result, title, exportDir, onExportDir);
      if (path) setNotice(`Exported to ${path}`);
    } catch (e) {
      setNotice(String(e));
    }
  }

  async function keep() {
    if (!result) return;
    try {
      await invoke("save_result", {
        args: {
          name: `${title} · ${new Date().toLocaleString()}`,
          source: output?.source ?? tab?.queryId ?? "",
          datasourceId: output?.datasourceId ?? null,
          params: output?.params ?? null,
          sql: result.executed_sql,
          result: { columns: result.columns, rows: result.rows, truncated: result.truncated },
        },
      });
      setNotice("Kept. It is listed under Saved results.");
      onKept();
    } catch (e) {
      setNotice(String(e));
    }
  }

  return (
    <section className="bottom">
      <div className="panel-tabs">
        <button className={view === "results" ? "panel-tab active" : "panel-tab"} onClick={() => setView("results")}>
          Results
        </button>
        <button className={view === "console" ? "panel-tab active" : "panel-tab"} onClick={() => setView("console")}>
          Console
        </button>
        <button className={view === "sql" ? "panel-tab active" : "panel-tab"} onClick={() => setView("sql")}>
          SQL
        </button>
        <button className={view === "runs" ? "panel-tab active" : "panel-tab"} onClick={() => setView("runs")}>
          Runs
          <span className="count-badge">{runs.length}</span>
        </button>
        {result && view === "results" && (
          <span className="meta">
            {result.rows.length} rows · {result.elapsed_ms} ms
            {result.truncated ? " · truncated" : ""}
          </span>
        )}
        <span className="spacer" />
        {result && view === "results" && (
          <div className="panel-actions">
            <button className="small ghost" title="Export this result as CSV" onClick={exportCsv}>
              <Download size={14} /> Export CSV
            </button>
            {tab?.kind !== "result" && (
              <button className="small ghost" title="Keep this result on disk" onClick={keep}>
                <Save size={14} /> Keep result
              </button>
            )}
            {tab && (
              <button className="small ghost" title="Ask the AI about this result" onClick={() => onAnalyze(result, tab, output?.datasourceId ?? null)}>
                <Sparkles size={14} /> Analyze
              </button>
            )}
          </div>
        )}
        {result && view === "console" && (
          <div className="panel-actions">
            {tab && (
              <button className="small ghost" title="Ask the AI about this output" onClick={() => onAnalyze(result, tab, output?.datasourceId ?? null)}>
                <Sparkles size={14} /> Analyze
              </button>
            )}
          </div>
        )}
        {result && view === "sql" && (
          <div className="panel-actions">
            <button
              className="small ghost"
              title="Copy the SQL"
              onClick={() => navigator.clipboard?.writeText(result.executed_sql).catch(() => undefined)}
            >
              <Copy size={14} /> Copy SQL
            </button>
          </div>
        )}
      </div>
      {notice && (
        <div className="panel-toast" role="status" onClick={() => setNotice(null)} title="Click to dismiss">
          {notice}
        </div>
      )}

      <div className="panel-body">
        {view === "results" && (
          <>
            {output?.error && <Banner kind="error">{output.error}</Banner>}
            {result && <ResultTable result={result} />}
            {!output?.error && !result && <div className="empty">Run a query to see its rows here.</div>}
          </>
        )}

        {view === "console" && (
          <Console result={result} error={output?.error ?? null} />
        )}

        {view === "sql" && (
          <>
            {result ? (
              <pre className="sent-sql">{result.executed_sql}</pre>
            ) : (
              <div className="empty">Run a query to see the SQL that was sent to the database.</div>
            )}
          </>
        )}

        {view === "runs" && (
          <>
            {!tab?.queryId && <div className="empty">Runs appear here for queries and saved statements.</div>}
            {loadingRuns && runs.length === 0 && <Spinner label="Loading runs…" />}
            {tab?.queryId && !loadingRuns && runs.length === 0 && <div className="empty">No runs yet.</div>}
            <ul className="runs">
              {runs.map((r) => (
                <li key={r.id}>
                  <button className="small ghost" onClick={() => onReuse(r)}>
                    Reuse values
                  </button>
                  <span className="meta">
                    {formatTime(r.ran_at)} ·{" "}
                    {r.ok ? `${r.rows ?? 0} rows · ${r.elapsed_ms ?? 0} ms` : <span className="error-inline">failed</span>}
                  </span>
                  <span className="meta small mono">{paramSummary(r.params_json)}</span>
                  {r.error && <div className="error-inline small">{r.error}</div>}
                </li>
              ))}
            </ul>
          </>
        )}
      </div>
    </section>
  );
}

/**
 * The output as a terminal shows it: black background, white text, stderr in a warm colour. A command
 * result has `stream` and `line` columns; any other result shows each row on one line.
 */
function Console(props: { result: QueryResult | null; error: string | null }) {
  const { result, error } = props;
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [result, error]);
  if (!result && !error) return <div className="empty">Run a server command to see its output here.</div>;
  const cmdStream = result ? result.columns.indexOf("stream") : -1;
  const cmdLine = result ? result.columns.indexOf("line") : -1;
  const lines: { stream: string; text: string }[] = [];
  if (result) {
    for (const row of result.rows) {
      if (cmdStream >= 0 && cmdLine >= 0) {
        lines.push({ stream: row[cmdStream] ?? "stdout", text: row[cmdLine] ?? "" });
      } else {
        lines.push({ stream: "stdout", text: row.map((v) => v ?? "NULL").join("\t") });
      }
    }
  }
  return (
    <div className="console" ref={ref}>
      {result?.executed_sql && <div className="console-cmd">$ {result.executed_sql.split("\n")[0]}</div>}
      {error && <div className="console-line stderr">{error}</div>}
      {lines.map((l, i) => (
        <div key={i} className={l.stream === "stderr" ? "console-line stderr" : l.stream === "info" ? "console-line info" : "console-line"}>
          {l.text}
        </div>
      ))}
      {result?.truncated && <div className="console-line info">… output truncated</div>}
    </div>
  );
}

function paramSummary(json: string): string {
  const p = parseParams(json);
  const parts = Object.entries(p).map(([k, v]) => `${k}=${v ?? "NULL"}`);
  return parts.length ? parts.join(", ") : "no parameters";
}
