import { useEffect, useState } from "react";
import { invoke } from "./api";
import { ToolButton } from "./EditorPane";

type Doc = { path: string; title: string; bytes: number; score: number };
type Prepared = { system: string; user: string; docs: { path: string; title: string; bytes: number }[]; tables: number; bytes: number };
type Proposal = { kind: string; text: string; params: string[]; warnings: string[]; needColumns: string[] };

const DOC_BUDGET = 40_000;

/**
 * The AI writes a statement or a command from the description. The request is shown and editable; the
 * answer is a proposal: nothing reaches the editor until "Accept", and the editor still needs Save.
 */
export function GenerateDialog(props: {
  kind: "sql" | "command" | "script";
  description: string;
  current: string;
  server: string;
  mappingPath: string;
  model: string;
  onAccept: (text: string) => void;
  onClose: () => void;
}) {
  const { kind, description, current, server, mappingPath, model, onAccept, onClose } = props;
  // The AI's own output area: an error stays here, in the panel, until the next attempt.
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [notes, setNotes] = useState("");
  const [docs, setDocs] = useState<Doc[]>([]);
  const [chosen, setChosen] = useState<string[]>([]);
  const [draft, setDraft] = useState<Prepared | null>(null);
  const [request, setRequest] = useState("");
  const [proposal, setProposal] = useState<Proposal | null>(null);
  const [busy, setBusy] = useState<"prepare" | "send" | null>(null);

  // The documents the system suggests for the description (and the notes), within the budget.
  useEffect(() => {
    invoke<Doc[]>("suggest_documents", { text: `${description} ${notes}`, budgetBytes: DOC_BUDGET })
      .then((d) => {
        setDocs(d);
        setChosen(d.map((x) => x.path));
      })
      .catch((e) => setError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function prepare(extra: string[] = []): Promise<Prepared | null> {
    setBusy("prepare");
    setError(null);
    setProposal(null);
    try {
      const p = await invoke<Prepared>("prepare_generation", {
        args: { kind, description, notes, current, server, mappingPath: mappingPath || null, docPaths: chosen, extraTables: extra },
      });
      setDraft(p);
      setRequest(p.user);
      return p;
    } catch (e) {
      setError(String(e));
      return null;
    } finally {
      setBusy(null);
    }
  }

  /**
   * Sends the request as it is in the editor (after a preview, so the text the user read). If the AI asks for
   * the columns of tables it needs, the request is built again with those columns and sent once more.
   */
  async function send(round = 0, extra: string[] = []): Promise<void> {
    let p: Prepared | null;
    let user: string;
    if (round === 0) setNote(null);
    if (round === 0 && draft) {
      p = draft;
      user = request;
    } else {
      p = await prepare(extra);
      if (!p) return;
      user = p.user;
    }
    setBusy("send");
    setError(null);
    try {
      const result = await invoke<Proposal>("send_generation", { args: { kind, system: p.system, user, model } });
      if (result.needColumns.length > 0 && round === 0) {
        setNote(`The AI asked for the columns of ${result.needColumns.join(", ")}; sending again with them.`);
        setBusy(null);
        return send(1, [...extra, ...result.needColumns]);
      }
      setProposal(result);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="gen-page" role="region" aria-label="Generate with AI">
      <div className="gen-head">
        <span>Generate {kind === "sql" ? "SQL" : kind === "script" ? "a script" : "a command"} with AI</span>
        <button className="icon-mini" title="Close this tab" onClick={onClose}>
          ×
        </button>
      </div>
      <div className="gen-body">
        <div className="analysis-label">What it should do (the Description) and any extra notes</div>
        <div className="gen-desc">{description.trim() || <span className="meta small">(empty: write the Description first)</span>}</div>
        <textarea
          className="analysis-question"
          rows={2}
          placeholder="Notes for the AI (optional): the containers, the period, the values you expect…"
          value={notes}
          onChange={(e) => {
            setNotes(e.target.value);
            setDraft(null);
          }}
        />
        <div className="analysis-label">System documents chosen ({chosen.length} of {docs.length})</div>
        <div className="gen-docs">
          {docs.length === 0 && <div className="meta small">No document matches the description.</div>}
          {docs.map((d) => (
            <label key={d.path} className="check analysis-doc" title={d.path}>
              <input
                type="checkbox"
                checked={chosen.includes(d.path)}
                onChange={(e) => {
                  setChosen(e.target.checked ? [...chosen, d.path] : chosen.filter((p) => p !== d.path));
                  setDraft(null);
                }}
              />
              <span>
                {d.title} <span className="meta small">{Math.round(d.bytes / 1024)} KB</span>
              </span>
            </label>
          ))}
        </div>
        <div className="gen-actions">
          <ToolButton icon="play" label="Preview the request" busy={busy === "prepare"} disabled={busy !== null} onClick={() => prepare()} />
          <ToolButton icon="download" label="Send to AI" primary busy={busy === "send"} disabled={busy !== null || !description.trim()} onClick={() => send()} />
        </div>
        {note && <div className="meta small">{note}</div>}
        {error && <div className="gen-error" role="alert">{error}</div>}
        {draft && (
          <>
            <div className="analysis-label">The request (edit it freely; only this text is sent)</div>
            <textarea className="analysis-text analysis-edit gen-request" value={request} spellCheck={false} onChange={(e) => setRequest(e.target.value)} />
          </>
        )}
        {proposal && (
          <div className="gen-proposal">
            <div className="analysis-label">Proposal (not in the editor yet)</div>
            {proposal.warnings.map((w) => (
              <div key={w} className="gen-warning">
                ⚠ {w}
              </div>
            ))}
            <pre className="analysis-text">{proposal.text}</pre>
            {proposal.params.length > 0 && <div className="meta small">Parameters: {proposal.params.map((p) => `:${p}`).join(", ")}</div>}
            <div className="gen-actions">
              <ToolButton
                icon="star"
                label="Accept: put it in the editor"
                primary
                onClick={() => {
                  onAccept(proposal.text);
                  onClose();
                }}
              />
              <ToolButton icon="x" label="Discard" onClick={() => setProposal(null)} />
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
