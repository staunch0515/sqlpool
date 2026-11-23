import { useEffect, useState } from "react";
import { invoke } from "./api";
import { Toast } from "./components";
import { EditorPane, ToolButton, ToolSep } from "./EditorPane";
import { Tab } from "./types";

/** What an analysis is about: the result, what produced it, and the question. */
export type AnalysisInput = {
  executed: string;
  description: string;
  columns: string[];
  rows: (string | null)[][];
  totalRows: number;
  truncated: boolean;
  /** The saved result this analysis is made from. */
  resultId: number | null;
};

type Doc = { path: string; title: string; bytes: number; score: number };
type Prepared = {
  system: string;
  user: string;
  docs: { path: string; title: string; bytes: number }[];
  sampleRows: number;
  redactedCells: number;
  bytes: number;
};

/** The size the documents may take together. The system picks within it. */
const DOC_BUDGET = 60_000;
const DEFAULT_QUESTION = "Summarize this result, and point out anything unusual or worth checking.";

/**
 * AI analysis of a result. The system suggests the documents about the tables in the result; the user can
 * change them, and the question. "Preview" shows exactly what would be sent. Only "Send" sends it, and sends
 * that same text.
 */
export function AnalysisTab(props: {
  tab: Tab;
  input: AnalysisInput | undefined;
  model: string;
  /** The tab was renamed or saved: the app replaces it (its key changes to the saved one). */
  onSaved: (t: Tab) => void;
  onDeleted: (key: string) => void;
  /** Something was saved or deleted: the Saved results panel reloads. */
  onChanged: () => void;
}) {
  const { tab, input, model, onSaved, onDeleted, onChanged } = props;
  const savedFromKey = tab.key.startsWith("analysis:saved-") ? Number(tab.key.slice("analysis:saved-".length)) : null;
  const [savedId, setSavedId] = useState<number | null>(savedFromKey);
  const [name, setName] = useState(tab.title.replace(/^Analysis: /, ""));
  const [source, setSource] = useState(input?.description ?? "");
  const [resultId, setResultId] = useState<number | null>(input?.resultId ?? null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [docs, setDocs] = useState<Doc[]>([]);
  const [chosen, setChosen] = useState<string[]>([]);
  const [question, setQuestion] = useState(DEFAULT_QUESTION);
  const [preview, setPreview] = useState<Prepared | null>(null);
  // The request as Markdown: what is sent is exactly this text, after any editing.
  const [draft, setDraft] = useState("");
  const [answer, setAnswer] = useState<string | null>(null);
  const [busy, setBusy] = useState<"preview" | "send" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // An analysis opened from the saved list: its name, request and answer come from the store.
  useEffect(() => {
    if (savedFromKey === null) return;
    invoke<{ name: string; source: string; request: string; answer: string; result_id: number | null }>("get_saved_analysis", { id: savedFromKey })
      .then((a) => {
        setName(a.name);
        setSource(a.source);
        setResultId(a.result_id);
        setDraft(a.request);
        setAnswer(a.answer);
      })
      .catch((e) => setError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.key]);

  async function save() {
    if (!name.trim()) return setError("Give the analysis a name first.");
    try {
      const id = await invoke<number>("save_analysis", {
        args: { id: savedId, name, source, request: draft, answer: answer ?? "", model, resultId },
      });
      setSavedId(id);
      setNotice("Saved. It is listed under Saved results.");
      onChanged();
      if (savedId === null) onSaved({ key: `analysis:saved-${id}`, kind: "analysis", title: name.trim(), queryId: "" });
    } catch (e) {
      setError(String(e));
    }
  }

  async function remove() {
    if (savedId === null) return;
    try {
      await invoke("delete_saved_analysis", { id: savedId });
      onChanged();
      onDeleted(tab.key);
    } catch (e) {
      setError(String(e));
    }
  }

  // The system's suggestion: the documents about this result's terms, within the budget.
  useEffect(() => {
    if (!input) return;
    // stream and line are the output's own columns, not words about the system.
    const text = [input.executed, input.description, ...input.columns.filter((c) => c !== "stream" && c !== "line")].join(" ");
    invoke<Doc[]>("suggest_documents", { text, budgetBytes: DOC_BUDGET })
      .then((d) => {
        setDocs(d);
        setChosen(d.map((x) => x.path));
      })
      .catch((e) => setError(String(e)));
  }, [tab.key]);

  function payload() {
    if (!input) throw new Error("the result is no longer here");
    return {
      executed: input.executed,
      description: input.description,
      columns: input.columns,
      rows: input.rows,
      totalRows: input.totalRows,
      truncated: input.truncated,
      question,
      docPaths: chosen,
    };
  }

  async function previewIt() {
    setBusy("preview");
    setError(null);
    setNotice(null);
    setAnswer(null);
    try {
      const p = await invoke<Prepared>("prepare_analysis", { args: payload() });
      setPreview(p);
      setDraft(p.user);
      return p;
    } catch (e) {
      setError(String(e));
      setPreview(null);
      return null;
    } finally {
      setBusy(null);
    }
  }

  /** Sends the text as it is in the editor: the request the user read, and edited if they changed it. */
  async function send() {
    const p = preview ?? (await previewIt());
    if (!p) return;
    // Right after a preview the editor has not re-rendered yet, so take the text the preview returned.
    const md = preview ? draft : p.user;
    setBusy("send");
    setError(null);
    try {
      const text = await invoke<string>("send_analysis", { args: { system: p.system, user: md, model } });
      setAnswer(text);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  const title = name.trim() || "Untitled analysis";
  return (
    <EditorPane
      crumbs={["Analysis", title]}
      title={title}
      actions={
        <>
          <ToolButton icon="play" label="Preview what is sent" busy={busy === "preview"} disabled={!input || busy !== null} onClick={previewIt} />
          <ToolButton icon="save" label="Save analysis" disabled={busy !== null || (!answer && !draft)} onClick={save} title="Keep this analysis: its name, what was sent and the answer" />
          <ToolButton
            icon="download"
            label={busy === "send" ? "Analyzing…" : "Send to AI"}
            primary
            busy={busy === "send"}
            disabled={!input || busy !== null || !preview}
            onClick={send}
            title="Send exactly the previewed text"
          />
          {savedId !== null && (
            <>
              <ToolSep />
              {confirmDelete ? (
                <ToolButton icon="x" label="Confirm delete" danger onClick={remove} />
              ) : (
                <ToolButton icon="trash" label="Delete saved analysis" onClick={() => setConfirmDelete(true)} />
              )}
            </>
          )}
          <ToolSep />
          <ToolButton
            icon="copy"
            label="Copy answer"
            disabled={!answer}
            onClick={() => {
              if (answer) navigator.clipboard?.writeText(answer).catch(() => undefined);
              setNotice("Copied.");
            }}
          />
        </>
      }
    >
      <div className="sql-info env-page analysis-page">
        <Toast kind={error ? "error" : "ok"} text={error ?? notice} onDismiss={() => { setError(null); setNotice(null); }} />
        <div className="analysis-grid">
          <div className="analysis-side">
            <div className="analysis-label">Name</div>
            <input
              className="analysis-name"
              value={name}
              placeholder="Name this analysis"
              onChange={(e) => setName(e.target.value)}
            />
            <div className="analysis-label">Question</div>
            <textarea
              className="analysis-question"
              rows={3}
              value={question}
              onChange={(e) => {
                setQuestion(e.target.value);
                setPreview(null);
              }}
            />
            <div className="analysis-label">
              System documents the system chose ({chosen.length} of {docs.length}, {Math.round(docs.filter((d) => chosen.includes(d.path)).reduce((n, d) => n + d.bytes, 0) / 1024)} KB)
            </div>
            <div className="analysis-docs">
              {docs.length === 0 && <div className="meta small">No document matches the terms of this result.</div>}
              {docs.map((d) => (
                <label key={d.path} className="check analysis-doc" title={d.path}>
                  <input
                    type="checkbox"
                    checked={chosen.includes(d.path)}
                    onChange={(e) => {
                      setChosen(e.target.checked ? [...chosen, d.path] : chosen.filter((p) => p !== d.path));
                      setPreview(null);
                    }}
                  />
                  <span>
                    {d.title} <span className="meta small">{Math.round(d.bytes / 1024)} KB</span>
                  </span>
                </label>
              ))}
            </div>
            {preview && (
              <div className="meta small analysis-facts">
                Will send {preview.sampleRows} sample rows, {preview.redactedCells} redacted values, {preview.docs.length} documents,{" "}
                {Math.round(preview.bytes / 1024)} KB in total.
              </div>
            )}
          </div>
          <div className="analysis-main">
            {answer !== null ? (
              <>
                <div className="analysis-label">Answer</div>
                <pre className="analysis-text">{answer}</pre>
              </>
            ) : preview ? (
              <>
                <div className="analysis-label">
                  What will be sent. Edit it freely: only this text is sent. Preview again to rebuild it from the question and documents.
                </div>
                <textarea
                  className="analysis-text analysis-sent analysis-edit"
                  value={draft}
                  spellCheck={false}
                  onChange={(e) => setDraft(e.target.value)}
                />
              </>
            ) : (
              <div className="meta small">Press “Preview what is sent” to see the request. Nothing leaves this machine until you press “Send to AI”.</div>
            )}
          </div>
        </div>
      </div>
    </EditorPane>
  );
}
