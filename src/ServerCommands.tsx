import { useEffect, useState } from "react";
import { invoke } from "./api";
import { Banner, BusyOverlay, IconButton, SideSearch, Toast } from "./components";
import { InfoLine, InfoTable, SqlPage, ToolButton, ToolSep } from "./EditorPane";
import { FilePlus, FolderPlus, Terminal, Trash } from "./icons";
import { SqlEditor } from "./SqlEditor";
import { SideTree, TreeNode, TreeTools, parentKeysOf, useTreeCollapse } from "./Tree";
import { Accepted, GenerateRequest, Output, QueryResult, SavedGroup, Tab } from "./types";

/** A saved shell command (see server_commands.rs). */
export type SavedCommand = {
  id: number;
  name: string;
  group: string;
  description: string;
  command: string;
  host: string;
  updated_at: number;
};

/** The `:name` parameters of a command: the same rule as the backend. */
export function commandParams(text: string): string[] {
  const out: string[] = [];
  const chars = Array.from(text);
  for (let i = 0; i < chars.length; i++) {
    const prev = i === 0 ? "" : chars[i - 1];
    const word = (c: string) => /[A-Za-z0-9_]/.test(c);
    if (chars[i] === ":" && !(prev && (word(prev) || prev === ":")) && /[A-Za-z_]/.test(chars[i + 1] ?? "")) {
      let j = i + 1;
      while (j < chars.length && /[A-Za-z0-9_]/.test(chars[j])) j++;
      const name = chars.slice(i + 1, j).join("");
      if (!out.includes(name)) out.push(name);
      i = j - 1;
    }
  }
  return out;
}

export function commandKey(id: number): string {
  return `command:${id}`;
}

/** Saved commands in folders, like Saved SQL: a folder per group, drag a command onto a folder to file it. */
export function CommandSide(props: {
  filter: string;
  onFilter: (value: string) => void;
  version: number;
  activeKey: string | null;
  onOpen: (item: SavedCommand) => void;
  onNew: (group: string) => void;
  onChanged: () => void;
}) {
  const { filter, onFilter, version, activeKey, onOpen, onNew, onChanged } = props;
  const [list, setList] = useState<SavedCommand[]>([]);
  const [groups, setGroups] = useState<SavedGroup[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [newName, setNewName] = useState("");
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const tree = useTreeCollapse("side-commands");

  useEffect(() => {
    (async () => {
      try {
        setList(await invoke<SavedCommand[]>("list_saved_commands"));
        setGroups(await invoke<SavedGroup[]>("list_command_groups"));
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
      await invoke("create_command_group", { name });
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
      await invoke("delete_command_group", { name });
      setPendingDelete(null);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  async function moveCommand(c: SavedCommand, group: string) {
    try {
      await invoke("save_saved_command", {
        args: { id: c.id, name: c.name, group, description: c.description, command: c.command, host: c.host },
      });
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  const t = filter.trim().toLowerCase();
  const names = [...new Set([...groups.map((g) => g.name), ...list.map((c) => c.group.trim())])].filter((n) => n !== "");
  const nodes: TreeNode[] = [
    ...names.map((name) => ({
      key: `g:${name}`,
      label: name,
      parent: null,
      kind: "group" as const,
      droppable: true,
      badge: <span className="count-badge">{list.filter((c) => c.group.trim() === name).length}</span>,
      actions: (
        <>
          <button className="icon-mini" title="New command in this folder" onClick={() => onNew(name)}>
            <FilePlus size={13} />
          </button>
          <button
            className={pendingDelete === name ? "icon-mini danger-text" : "icon-mini"}
            title={pendingDelete === name ? "Click again to delete the folder" : "Delete folder (commands move to the top)"}
            onClick={() => deleteGroup(name)}
          >
            {pendingDelete === name ? "Confirm" : <Trash size={13} />}
          </button>
        </>
      ),
    })),
    ...list
      .filter((c) => !t || [c.name, c.description, c.command].some((x) => x.toLowerCase().includes(t)))
      .map((c) => ({
        key: commandKey(c.id),
        label: c.name,
        parent: c.group.trim() ? `g:${c.group.trim()}` : null,
        kind: "item" as const,
        icon: <Terminal size={14} />,
        tone: "saved" as const,
      })),
  ];

  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search server commands" />
      <div className="side-toolbar">
        <IconButton icon={<FilePlus size={15} />} title="New command" onClick={() => onNew("")} />
        <IconButton icon={<FolderPlus size={15} />} title="New folder" onClick={() => setAdding(true)} />
        <span className="side-count">{list.length} commands</span>
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
        activeKey={activeKey}
        onOpen={(key) => {
          const c = list.find((x) => commandKey(x.id) === key);
          if (c) onOpen(c);
        }}
        onMove={(key, parent) => {
          const c = list.find((x) => commandKey(x.id) === key);
          if (c) moveCommand(c, parent ? parent.slice(2) : "");
        }}
        dropToTop
        filter={filter}
        empty={filter ? "No commands match this search." : "No saved commands yet."}
        collapsed={tree.collapsed}
        onToggle={tree.toggle}
      />
    </>
  );
}

type Draft = { name: string; group: string; description: string; command: string; host: string };

/**
 * A saved command in its tab, laid out like a saved statement: breadcrumb, toolbar (save and run, save,
 * copy, delete), the properties, the parameters and the server, then the command. The output goes to the
 * results area below.
 */
export function CommandEditor(props: {
  tab: Tab;
  model: string;
  onGenerate: (req: GenerateRequest) => void;
  accepted: Accepted | null;
  onOutput: (key: string, out: Output) => void;
  onChanged: () => void;
  onSaved: (tab: Tab) => void;
  onDeleted: (key: string) => void;
  onCursor: (line: number, column: number) => void;
}) {
  const { tab, onGenerate, accepted, onOutput, onChanged, onSaved, onDeleted, onCursor } = props;
  const id = tab.id ?? null;
  const [loaded, setLoaded] = useState(id === null);
  const [draft, setDraft] = useState<Draft>({ name: "", group: tab.group ?? "", description: "", command: "", host: "quizbuks-staging" });
  const [values, setValues] = useState<Record<string, string>>({});
  const [running, setRunning] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const params = commandParams(draft.command);

  function openGenerate() {
    onGenerate({
      kind: "command",
      description: draft.description,
      current: draft.command,
      server: draft.host,
      mappingPath: "",
      originKey: tab.key,
    });
  }

  // An accepted proposal goes into the draft; the command still has to be saved.
  useEffect(() => {
    if (!accepted || accepted.key !== tab.key) return;
    setDraft((d) => ({ ...d, command: accepted.text }));
    setNotice("Proposal accepted. Review it, then Save.");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accepted?.nonce]);

  useEffect(() => {
    if (id === null) return;
    (async () => {
      try {
        const all = await invoke<SavedCommand[]>("list_saved_commands");
        const c = all.find((x) => x.id === id);
        if (c) setDraft({ name: c.name, group: c.group, description: c.description, command: c.command, host: c.host });
      } catch (e) {
        setError(String(e));
      } finally {
        setLoaded(true);
      }
    })();
  }, [tab.key]);

  async function persist(): Promise<number> {
    const newId = await invoke<number>("save_saved_command", {
      args: { id, name: draft.name, group: draft.group, description: draft.description, command: draft.command, host: draft.host },
    });
    if (id === null) {
      onSaved({ key: commandKey(newId), kind: "command", title: draft.name.trim() || "Untitled", queryId: `command:${newId}`, id: newId });
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
      outKey = commandKey(newId);
      const args: Record<string, string | null> = {};
      for (const name of params) {
        const v = values[name]?.trim() ?? "";
        args[name] = v === "" ? null : v;
      }
      const result = await invoke<QueryResult>("run_saved_command", {
        args: { id: newId, host: draft.host.trim(), params: args },
      });
      onOutput(outKey, { result, error: null, source: `command:${newId}`, datasourceId: null });
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
      await invoke("delete_saved_command", { id });
      onDeleted(tab.key);
      onChanged();
    } catch (e) {
      setError(String(e));
    }
  }

  const title = draft.name.trim() || "New command";
  if (!loaded) return <BusyOverlay label={`Opening ${tab.title}…`} />;

  return (
    <SqlPage
      crumbs={draft.group.trim() ? ["Server commands", draft.group.trim(), title] : ["Server commands", title]}
      title={title}
      actions={
        <>
          <ToolButton
            icon="play"
            label={running ? "Running…" : "Save and run"}
            primary
            busy={running}
            disabled={running || !draft.command.trim() || !draft.host.trim()}
            onClick={saveAndRun}
            title="Save, then run on the server. The output goes to the results area."
          />
          <ToolButton icon="save" label={saving ? "Saving…" : "Save"} busy={saving} disabled={running} onClick={save} />
          <ToolButton icon="sparkles" label="Generate with AI" onClick={() => openGenerate()} title="Write the command from the Description with AI" />
          <ToolSep />
          <ToolButton icon="copy" label="Copy command" onClick={() => navigator.clipboard?.writeText(draft.command).catch(() => undefined)} />
          {id !== null &&
            (confirmDelete ? (
              <ToolButton icon="x" label="Confirm delete" danger onClick={remove} />
            ) : (
              <ToolButton icon="trash" label="Delete" onClick={() => setConfirmDelete(true)} />
            ))}
        </>
      }
      fields={[
        { label: "Name", value: draft.name, placeholder: "e.g. Disk usage on staging", onChange: (v) => setDraft({ ...draft, name: v }) },
        { label: "Folder", value: draft.group, placeholder: "Top of the tree", onChange: (v) => setDraft({ ...draft, group: v }) },
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
            <InfoLine label="Server (ssh)">
              <input value={draft.host} placeholder="quizbuks-staging" onChange={(e) => setDraft({ ...draft, host: e.target.value })} />
            </InfoLine>
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
      }
      sql={draft.command}
      onSqlChange={(command) => setDraft((d) => ({ ...d, command }))}
      onRun={saveAndRun}
      onCursor={onCursor}
      sqlEditor={(p) => <SqlEditor {...p} />}
      overlay={running ? <BusyOverlay label="Running on the server…" /> : null}
    />
  );
}
