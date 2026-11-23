import { useEffect, useMemo, useState } from "react";
import { invoke } from "./api";
import { BusyOverlay, Toast } from "./components";
import { InfoLine, InfoTable, SqlPage, ToolButton, ToolSep } from "./EditorPane";
import { SqlEditor } from "./SqlEditor";
import { DataSourceSelect } from "./DataSources";
import { Accepted, Datasource, GenerateRequest, Output, parseParams, QueryResult, resolveDatasource, Reuse, SavedSql, Settings, Tab, usedParams } from "./types";

type Draft = { name: string; group: string; description: string; sql: string };

/**
 * Editor tab for a saved statement, or a new one (id null). Layout: breadcrumb (folder › statement),
 * toolbar (save, save and run, delete), the properties and parameter strips, then the SQL editor.
 * The first save of a new statement turns its draft tab into the saved one.
 */
export function SavedEditor(props: {
  tab: Tab;
  settings: Settings;
  reuse: Reuse | null;
  onOutput: (key: string, out: Output) => void;
  onChanged: () => void;
  onSaved: (tab: Tab) => void;
  onDeleted: (key: string) => void;
  onCursor: (line: number, column: number) => void;
  /** "Save as": a copy of this statement, opened, with the current parameter values. */
  onCopied: (tab: Tab, params: Record<string, string | null>) => void;
  /** The active environment's values: they fill the parameters first. */
  env: Record<string, string>;
  /** Opens the Generate tab for this statement. */
  onGenerate: (req: GenerateRequest) => void;
  /** A proposal the user accepted in the Generate tab, for this statement. */
  accepted: Accepted | null;
  datasources: Datasource[];
  defaultDs: number | null;
  envDs: number | null;
}) {
  const { tab, settings, reuse, onOutput, onChanged, onSaved, onDeleted, onCursor, onCopied, env, datasources, defaultDs, envDs, onGenerate, accepted } = props;
  const [dsChoice, setDsChoice] = useState<number | null>(null);
  const resolvedDs = resolveDatasource(dsChoice, envDs, defaultDs);
  const [copying, setCopying] = useState(false);
  const [loaded, setLoaded] = useState(tab.id == null);
  const [draft, setDraft] = useState<Draft>({ name: "", group: tab.group ?? "", description: "", sql: "" });
  const [values, setValues] = useState<Record<string, string>>({});
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [saving, setSaving] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);

  const params = useMemo(() => usedParams(draft.sql), [draft.sql]);

  function openGenerate() {
    onGenerate({
      kind: "sql",
      description: draft.description,
      current: draft.sql,
      server: "",
      mappingPath: settings.mappingPath,
      originKey: tab.key,
    });
  }

  // An accepted proposal goes into the draft; the statement still has to be saved.
  useEffect(() => {
    if (!accepted || accepted.key !== tab.key) return;
    setDraft((d) => ({ ...d, sql: accepted.text }));
    setNotice("Proposal accepted. Review it, then Save.");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accepted?.nonce]);

  // Load the statement when its tab opens (not on every render, or unsaved edits would be lost).
  useEffect(() => {
    (async () => {
      try {
        const all = await invoke<SavedSql[]>("list_saved_sql");
        if (tab.id != null) {
          const item = all.find((s) => s.id === tab.id);
          if (item) {
            setDraft({ name: item.name, group: item.group, description: item.description, sql: item.sql });
            setDsChoice(item.datasource_id);
          }
        }
      } catch (e) {
        setError(String(e));
      } finally {
        setLoaded(true);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.key]);

  // Parameters that the active environment defines take its values (when loaded, or when the environment changes).
  const envJson = JSON.stringify(env);
  useEffect(() => {
    if (!loaded) return;
    const fromEnv = JSON.parse(envJson) as Record<string, string>;
    setValues((prev) => {
      const next = { ...prev };
      for (const name of usedParams(draft.sql)) {
        if (name in fromEnv) next[name] = fromEnv[name];
      }
      return next;
    });
    // Only on load and on an environment change: typing in the SQL must not overwrite the values.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded, envJson]);

  // "Open" from the run history: restore the values the run used.
  useEffect(() => {
    if (!reuse || reuse.key !== tab.key) return;
    const saved = parseParams(reuse.params);
    const next: Record<string, string> = {};
    for (const [name, v] of Object.entries(saved)) if (name !== "__asOf") next[name] = v ?? "";
    setValues(next);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reuse?.nonce]);

  /** Writes the draft to the database and returns its id. */
  async function persist(): Promise<number> {
    const id = await invoke<number>("save_saved_sql", {
      id: tab.id ?? null,
      name: draft.name,
      group: draft.group,
      description: draft.description,
      sql: draft.sql,
    });
    await invoke("set_saved_datasource", { id, datasourceId: dsChoice });
    if (tab.id == null) {
      onSaved({
        key: `saved:${id}`,
        kind: "saved",
        title: draft.name.trim() || "Untitled",
        queryId: `saved:${id}`,
        id,
      });
    }
    onChanged();
    return id;
  }

  async function save() {
    setError(null);
    setNotice(null);
    setSaving(true);
    try {
      await persist();
      setNotice("Saved.");
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  async function saveAndRun() {
    if (running) return;
    setRunning(true);
    setError(null);
    setNotice(null);
    let outKey = tab.key;
    try {
      const id = await persist();
      outKey = `saved:${id}`;
      const args: Record<string, string | null> = {};
      for (const name of params) {
        const v = values[name]?.trim() ?? "";
        args[name] = v === "" ? null : v;
      }
      if (resolvedDs === null) throw new Error("No data source: choose one here, in the environment, or as the default.");
      const result = await invoke<QueryResult>("run_saved_sql", { args: { id, datasourceId: resolvedDs, params: args } });
      onOutput(outKey, { result, error: null, source: `saved:${id}`, datasourceId: resolvedDs });
    } catch (e) {
      onOutput(outKey, { result: null, error: String(e) });
    } finally {
      setRunning(false);
      onChanged();
    }
  }

  /** Save as: a copy under a new id, opened with the current values. */
  async function copy() {
    if (copying) return;
    setCopying(true);
    setError(null);
    try {
      const id = await invoke<number>("save_saved_sql", {
        id: null,
        name: `${draft.name.trim() || "Untitled"} (copy)`,
        group: draft.group,
        description: draft.description,
        sql: draft.sql,
      });
      const params: Record<string, string | null> = {};
      for (const name of usedParams(draft.sql)) params[name] = values[name]?.trim() ? values[name] : null;
      onCopied(
        { key: `saved:${id}`, kind: "saved", title: `${draft.name.trim() || "Untitled"} (copy)`, queryId: `saved:${id}`, id },
        params,
      );
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setCopying(false);
    }
  }

  async function remove() {
    if (tab.id == null) return;
    try {
      await invoke("delete_saved_sql", { id: tab.id });
      onDeleted(tab.key);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  const title = draft.name.trim() || "New statement";

  // A saved statement is covered until it has loaded, so the open is visibly in progress.
  if (!loaded) return <BusyOverlay label={`Opening ${tab.title}…`} />;

  return (
    <SqlPage
      crumbs={draft.group.trim() ? ["Saved SQL", draft.group.trim(), title] : ["Saved SQL", title]}
      title={title}
      actions={
        <>
          <ToolButton
            icon="play"
            label={running ? "Running…" : "Save and run"}
            primary
            busy={running}
            onClick={saveAndRun}
            disabled={running || resolvedDs === null || !draft.sql.trim()}
            title={resolvedDs !== null ? "Save, then run read-only (Ctrl+Enter)" : "Choose a data source for this statement"}
          />
          <ToolButton icon="saveAs" label={copying ? "Copying…" : "Save as"} onClick={copy} busy={copying} disabled={running} title="Save a copy of this statement, with the current parameter values" />
          <ToolButton icon="save" label={saving ? "Saving…" : "Save"} onClick={save} busy={saving} disabled={running} />
          <ToolButton icon="sparkles" label="Generate with AI" onClick={openGenerate} title="Write the SQL from the Description with AI" />
          <ToolSep />
          <ToolButton icon="copy" label="Copy SQL" onClick={() => navigator.clipboard?.writeText(draft.sql).catch(() => undefined)} />
          <ToolButton icon="hash" label="Copy id" disabled={tab.id == null} onClick={() => navigator.clipboard?.writeText(String(tab.id ?? "")).catch(() => undefined)} />
          <ToolSep />
          <ToolButton icon="star" label="Favourite" disabled title="Favourites are for catalog queries" onClick={() => undefined} />
          {tab.id != null &&
            (confirmDelete ? (
              <ToolButton icon="x" label="Confirm delete" danger onClick={remove} />
            ) : (
              <ToolButton icon="trash" label="Delete" onClick={() => setConfirmDelete(true)} />
            ))}
        </>
      }
      fields={[
        { label: "Name", value: draft.name, placeholder: "e.g. Open orders by status", onChange: (v) => setDraft({ ...draft, name: v }) },
        { label: "Description", value: draft.description, placeholder: "What it is for", multiline: true, onChange: (v) => setDraft({ ...draft, description: v }) },
      ]}
      banners={
        <>
          <Toast kind={error ? "error" : "ok"} text={error ?? notice} onDismiss={() => { setError(null); setNotice(null); }} />
        </>
      }
      params={
        <div className="params-block">
          <InfoTable>
            <InfoLine label="Data source">
              <DataSourceSelect
                value={dsChoice}
                datasources={datasources}
                onChange={(v) => {
                  setDsChoice(v);
                  if (tab.id != null) {
                    invoke("set_saved_datasource", { id: tab.id, datasourceId: v }).catch((e) =>
                      onOutput(tab.key, { result: null, error: String(e) }),
                    );
                  }
                }}
              />
            </InfoLine>
            {params.map((name) => (
              <InfoLine key={name} label={`:${name}`}>
                <input value={values[name] ?? ""} placeholder="empty = NULL" onChange={(e) => setValues({ ...values, [name]: e.target.value })} />
              </InfoLine>
            ))}
          </InfoTable>
        </div>
      }
      sql={draft.sql}
      onSqlChange={(sql) => setDraft((d) => ({ ...d, sql }))}
      onRun={saveAndRun}
      onCursor={onCursor}
      sqlEditor={(p) => <SqlEditor {...p} />}
      overlay={
        running || saving || copying ? <BusyOverlay label={running ? "Running…" : saving ? "Saving…" : "Copying…"} /> : null
      }
    />
  );
}
