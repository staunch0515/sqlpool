import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { invoke } from "./api";
import { Banner, BusyOverlay, IconButton, SideSearch, Toast } from "./components";
import { InfoLine, InfoTable, SqlPage, ToolButton, ToolSep } from "./EditorPane";
import { Download, FileCode, FilePlus } from "./icons";
import { SqlEditor } from "./SqlEditor";
import { SideTree, TreeNode, TreeTools, parentKeysOf, useTreeCollapse } from "./Tree";
import { Accepted, GenerateRequest, Output, QueryResult, Tab } from "./types";
import { commandParams } from "./ServerCommands";

/** A local bash script kept in the app (see local_script.rs). */
export type LocalScript = {
  id: number;
  name: string;
  group: string;
  description: string;
  body: string;
  updated_at: number;
};

export function scriptKey(id: number): string {
  return `script:${id}`;
}

/** Local scripts in folders, like server commands. Drag a script onto a folder to file it. */
export function LocalScriptSide(props: {
  filter: string;
  onFilter: (value: string) => void;
  version: number;
  activeKey: string | null;
  onOpen: (item: LocalScript) => void;
  onNew: (group: string) => void;
  onChanged: () => void;
}) {
  const { filter, onFilter, version, activeKey, onOpen, onNew, onChanged } = props;
  const [list, setList] = useState<LocalScript[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const tree = useTreeCollapse("side-scripts");

  useEffect(() => {
    (async () => {
      try {
        setList(await invoke<LocalScript[]>("list_local_scripts"));
        setError(null);
        setLoaded(true);
      } catch (e) {
        setError(String(e));
      }
    })();
  }, [version]);

  async function moveScript(s: LocalScript, group: string) {
    try {
      await invoke("save_local_script", {
        args: { id: s.id, name: s.name, group, description: s.description, body: s.body },
      });
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  /** Writes every script as a .sh file into a folder the user picks. */
  async function exportAll() {
    try {
      const dir = await open({ directory: true, multiple: false, title: "Choose the folder for the scripts" });
      if (typeof dir !== "string") return;
      const n = await invoke<number>("export_local_scripts", { dir });
      setError(null);
      setNotice(`Exported ${n} script${n === 1 ? "" : "s"} to ${dir}`);
    } catch (e) {
      setError(String(e));
    }
  }

  const t = filter.trim().toLowerCase();
  const names = [...new Set(list.map((s) => s.group.trim()))].filter((n) => n !== "").sort();
  const nodes: TreeNode[] = [
    ...names.map((name) => ({
      key: `g:${name}`,
      label: name,
      parent: null,
      kind: "group" as const,
      droppable: true,
      badge: <span className="count-badge">{list.filter((s) => s.group.trim() === name).length}</span>,
      actions: (
        <button className="icon-mini" title="New script in this folder" onClick={() => onNew(name)}>
          <FilePlus size={13} />
        </button>
      ),
    })),
    ...list
      .filter((s) => !t || [s.name, s.description, s.body].some((x) => x.toLowerCase().includes(t)))
      .map((s) => ({
        key: scriptKey(s.id),
        label: s.name,
        parent: s.group.trim() ? `g:${s.group.trim()}` : null,
        kind: "item" as const,
        icon: <FileCode size={14} />,
        tone: "saved" as const,
      })),
  ];

  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search local scripts" />
      <div className="side-toolbar">
        <IconButton icon={<FilePlus size={15} />} title="New script" onClick={() => onNew("")} />
        <IconButton icon={<Download size={15} />} title="Export all scripts as .sh files" onClick={exportAll} />
        <span className="side-count">{list.length} scripts</span>
        <TreeTools onExpand={tree.expandAll} onCollapse={() => tree.collapseAll(parentKeysOf(nodes))} />
      </div>
      {error && <Banner kind="error">{error}</Banner>}
      {notice && !error && <Banner kind="ok">{notice}</Banner>}
      <SideTree
        loading={!loaded}
        nodes={nodes}
        activeKey={activeKey}
        onOpen={(key) => {
          const s = list.find((x) => scriptKey(x.id) === key);
          if (s) onOpen(s);
        }}
        onMove={(key, parent) => {
          const s = list.find((x) => scriptKey(x.id) === key);
          if (s) moveScript(s, parent ? parent.slice(2) : "");
        }}
        dropToTop
        filter={filter}
        empty={filter ? "No scripts match this search." : "No local scripts yet."}
        collapsed={tree.collapsed}
        onToggle={tree.toggle}
      />
    </>
  );
}

type Draft = { name: string; group: string; description: string; body: string };

/**
 * A local script in its tab, laid out like a server command: breadcrumb, toolbar (save and run, save,
 * generate, copy, delete), the properties, the parameters, then the script. Parameters (`:name`) take the
 * active environment's values first. The output goes to the results area below.
 */
export function LocalScriptEditor(props: {
  tab: Tab;
  /** The active environment's values: they fill the parameters first. */
  env: Record<string, string>;
  onGenerate: (req: GenerateRequest) => void;
  /** A proposal the user accepted in the Generate tab, for this script. */
  accepted: Accepted | null;
  onOutput: (key: string, out: Output) => void;
  onChanged: () => void;
  onSaved: (tab: Tab) => void;
  onDeleted: (key: string) => void;
  onCursor: (line: number, column: number) => void;
}) {
  const { tab, env, onGenerate, accepted, onOutput, onChanged, onSaved, onDeleted, onCursor } = props;
  const id = tab.id ?? null;
  const [loaded, setLoaded] = useState(id === null);
  const [draft, setDraft] = useState<Draft>({ name: "", group: tab.group ?? "", description: "", body: "" });
  const [values, setValues] = useState<Record<string, string>>({});
  const [running, setRunning] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const params = commandParams(draft.body);

  useEffect(() => {
    if (id === null) return;
    (async () => {
      try {
        const all = await invoke<LocalScript[]>("list_local_scripts");
        const s = all.find((x) => x.id === id);
        if (s) setDraft({ name: s.name, group: s.group, description: s.description, body: s.body });
      } catch (e) {
        setError(String(e));
      } finally {
        setLoaded(true);
      }
    })();
  }, [tab.key]);

  // Parameters that the active environment defines take its values (on load and on an environment change).
  const envJson = JSON.stringify(env);
  useEffect(() => {
    if (!loaded) return;
    const fromEnv = JSON.parse(envJson) as Record<string, string>;
    setValues((prev) => {
      const next = { ...prev };
      for (const name of commandParams(draft.body)) {
        if (name in fromEnv) next[name] = fromEnv[name];
      }
      return next;
    });
    // Typing in the script must not overwrite the values, so only the load and the environment trigger this.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [loaded, envJson]);

  // An accepted proposal goes into the draft; the script still has to be saved.
  useEffect(() => {
    if (!accepted || accepted.key !== tab.key) return;
    setDraft((d) => ({ ...d, body: accepted.text }));
    setNotice("Proposal accepted. Review it, then Save.");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accepted?.nonce]);

  function openGenerate() {
    onGenerate({
      kind: "script",
      description: draft.description,
      current: draft.body,
      server: "",
      mappingPath: "",
      originKey: tab.key,
    });
  }

  async function persist(): Promise<number> {
    const newId = await invoke<number>("save_local_script", {
      args: { id, name: draft.name, group: draft.group, description: draft.description, body: draft.body },
    });
    if (id === null) {
      onSaved({ key: scriptKey(newId), kind: "script", title: draft.name.trim() || "Untitled", queryId: `script:${newId}`, id: newId });
    }
    onChanged();
    return newId;
  }

  async function save() {
    setSaving(true);
    setError(null);
    setNotice(null);
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
      const newId = await persist();
      outKey = scriptKey(newId);
      const args: Record<string, string | null> = {};
      for (const name of params) {
        const v = values[name]?.trim() ?? "";
        args[name] = v === "" ? null : v;
      }
      const result = await invoke<QueryResult>("run_local_script", { args: { id: newId, params: args } });
      onOutput(outKey, { result, error: null, source: `script:${newId}`, datasourceId: null });
    } catch (e) {
      onOutput(outKey, { result: null, error: String(e) });
    } finally {
      setRunning(false);
      onChanged();
    }
  }

  async function remove() {
    if (id === null) return;
    try {
      await invoke("delete_local_script", { id });
      onDeleted(tab.key);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  const title = draft.name.trim() || "New script";
  if (!loaded) return <BusyOverlay label={`Opening ${tab.title}…`} />;

  return (
    <SqlPage
      crumbs={draft.group.trim() ? ["Local scripts", draft.group.trim(), title] : ["Local scripts", title]}
      title={title}
      actions={
        <>
          <ToolButton
            icon="play"
            label={running ? "Running…" : "Save and run"}
            primary
            busy={running}
            disabled={running || !draft.body.trim()}
            onClick={saveAndRun}
            title="Save, then run on this Mac. The output goes to the results area."
          />
          <ToolButton icon="save" label={saving ? "Saving…" : "Save"} busy={saving} disabled={running} onClick={save} />
          <ToolButton icon="sparkles" label="Generate with AI" onClick={() => openGenerate()} title="Write the script from the Description with AI" />
          <ToolSep />
          <ToolButton icon="copy" label="Copy script" onClick={() => navigator.clipboard?.writeText(draft.body).catch(() => undefined)} />
          {id !== null &&
            (confirmDelete ? (
              <ToolButton icon="x" label="Confirm delete" danger onClick={remove} />
            ) : (
              <ToolButton icon="trash" label="Delete" onClick={() => setConfirmDelete(true)} />
            ))}
        </>
      }
      fields={[
        { label: "Name", value: draft.name, placeholder: "e.g. Disk usage", onChange: (v) => setDraft({ ...draft, name: v }) },
        { label: "Folder", value: draft.group, placeholder: "Top of the tree", onChange: (v) => setDraft({ ...draft, group: v }) },
        { label: "Description", value: draft.description, placeholder: "What it is for", multiline: true, onChange: (v) => setDraft({ ...draft, description: v }) },
      ]}
      banners={<Toast kind={error ? "error" : "ok"} text={error ?? notice} onDismiss={() => { setError(null); setNotice(null); }} />}
      params={
        params.length > 0 ? (
          <div className="params-block">
            <InfoTable>
              {params.map((name) => (
                <InfoLine key={name} label={`:${name}`}>
                  <input
                    value={values[name] ?? ""}
                    placeholder="empty = empty string"
                    onChange={(e) => setValues({ ...values, [name]: e.target.value })}
                  />
                </InfoLine>
              ))}
            </InfoTable>
          </div>
        ) : null
      }
      sql={draft.body}
      onSqlChange={(body) => setDraft((d) => ({ ...d, body }))}
      onRun={saveAndRun}
      onCursor={onCursor}
      sqlEditor={(p) => <SqlEditor {...p} language="shell" />}
      overlay={running ? <BusyOverlay label="Running on this Mac…" /> : null}
    />
  );
}
