import { useState } from "react";
import { invoke } from "./api";
import { IconButton, PickMenu, SideSearch, Toast } from "./components";
import { EditorPane, InfoLine, InfoTable, ToolButton, ToolSep } from "./EditorPane";
import { Database, Download, Plus } from "./icons";
import { SideTree } from "./Tree";
import { Datasource, Tab } from "./types";

/** The key for a data source's tab. */
export function dsKey(id: number): string {
  return `ds:${id}`;
}

/** Side list of data sources. The default one is marked; over-ssh ones say so. */
export function DataSourcesSide(props: {
  datasources: Datasource[];
  filter: string;
  onFilter: (value: string) => void;
  activeKey: string | null;
  defaultDs: number | null;
  onOpen: (id: number) => void;
  onNew: () => void;
  onBackup: () => void;
}) {
  const { datasources, filter, onFilter, activeKey, defaultDs, onOpen, onNew, onBackup } = props;
  return (
    <>
      <SideSearch value={filter} onChange={onFilter} placeholder="Search data sources" />
      <div className="side-toolbar">
        <IconButton icon={<Plus size={15} />} title="New data source" onClick={onNew} />
        <IconButton icon={<Download size={15} />} title="Back up a server database into a new data source" onClick={onBackup} />
        <span className="side-count">{datasources.length} data sources</span>
      </div>
      <SideTree
        nodes={datasources.map((d) => ({
          key: dsKey(d.id),
          label: d.name,
          parent: null,
          kind: "item" as const,
          tone: "ds" as const,
          icon: <Database size={14} />,
          badge: (
            <>
              {d.id === defaultDs && <span className="env-in-use">default</span>}
              {d.via_ssh && <span className="count-badge">ssh</span>}
            </>
          ),
        }))}
        activeKey={activeKey}
        onOpen={(key) => {
          const d = datasources.find((x) => dsKey(x.id) === key);
          if (d) onOpen(d.id);
        }}
        filter={filter}
        empty={datasources.length === 0 ? "No data sources yet. Add one with +." : "No match."}
        collapsed={[]}
        onToggle={() => undefined}
      />
    </>
  );
}

type Draft = {
  name: string;
  host: string;
  port: string;
  database: string;
  user: string;
  viaSsh: boolean;
  sshHost: string;
  container: string;
};

function draftOf(d: Datasource | null): Draft {
  return {
    name: d?.name ?? "",
    host: d?.host ?? "",
    port: String(d?.port ?? 5432),
    database: d?.database ?? "",
    user: d?.user ?? "",
    viaSsh: d?.via_ssh ?? false,
    sshHost: d?.ssh_host ?? "",
    container: d?.container ?? "",
  };
}

/**
 * Editor tab for one data source. The password is typed here and goes to the keychain; the empty
 * password field keeps the saved one. "Run over ssh" runs the query inside the container on the server.
 */
export function DataSourceEditor(props: {
  tab: Tab;
  datasources: Datasource[];
  defaultDs: number | null;
  onChanged: () => void;
  onRenamed: (oldKey: string, t: Tab) => void;
  onDeleted: (key: string) => void;
  onDefault: (id: number | null) => void;
}) {
  const { tab, datasources, defaultDs, onChanged, onRenamed, onDeleted, onDefault } = props;
  const id = tab.key.startsWith("ds:new") ? null : Number(tab.key.slice(3));
  const original = id === null ? null : datasources.find((d) => d.id === id) ?? null;
  const [draft, setDraft] = useState<Draft>(() => draftOf(original));
  const [loadedFor, setLoadedFor] = useState<string | null>(original ? tab.key : null);
  const [password, setPassword] = useState("");
  const [clearPassword, setClearPassword] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [confirmDelete, setConfirmDelete] = useState(false);

  // The first time this tab's data source is known, fill the form from it.
  if (original && loadedFor !== tab.key) {
    setLoadedFor(tab.key);
    setDraft(draftOf(original));
  }

  const title = draft.name.trim() || "New data source";
  const isNew = original === null;

  function set<K extends keyof Draft>(k: K, v: Draft[K]) {
    setDraft((d) => ({ ...d, [k]: v }));
  }

  async function save() {
    setError(null);
    setNotice(null);
    try {
      const savedId = await invoke<number>("save_datasource", {
        args: {
          id: id,
          name: draft.name,
          host: draft.host,
          port: Number(draft.port) || 5432,
          database: draft.database,
          user: draft.user,
          viaSsh: draft.viaSsh,
          sshHost: draft.sshHost,
          container: draft.container,
          password: clearPassword ? "" : password !== "" ? password : null,
        },
      });
      setPassword("");
      setClearPassword(false);
      onChanged();
      if (dsKey(savedId) !== tab.key) {
        onRenamed(tab.key, { key: dsKey(savedId), kind: "ds", title: draft.name.trim(), queryId: "" });
      } else {
        setNotice("Saved.");
      }
    } catch (e) {
      setError(String(e));
    }
  }

  async function test() {
    if (id === null) return setError("Save the data source first.");
    setError(null);
    setNotice(null);
    try {
      const version = await invoke<string>("test_datasource", { id });
      setNotice(`Connected. ${version}`);
    } catch (e) {
      setError(String(e));
    }
  }

  async function remove() {
    if (id === null) return;
    try {
      await invoke("delete_datasource", { id });
      if (defaultDs === id) onDefault(null);
      onChanged();
      onDeleted(tab.key);
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <EditorPane
      crumbs={["Data sources", title]}
      title={title}
      actions={
        <>
          <ToolButton icon="save" label="Save" primary onClick={save} />
          <ToolButton icon="play" label="Test connection" disabled={isNew} onClick={test} />
          <ToolButton
            icon={defaultDs !== null && defaultDs === id ? "starFilled" : "star"}
            label={defaultDs !== null && defaultDs === id ? "Default" : "Use as default"}
            disabled={isNew}
            title="Statements with no data source of their own run on the default one"
            onClick={() => id !== null && onDefault(defaultDs === id ? null : id)}
          />
          <ToolSep />
          {!isNew &&
            (confirmDelete ? (
              <ToolButton icon="x" label="Confirm delete" danger onClick={remove} />
            ) : (
              <ToolButton icon="trash" label="Delete" onClick={() => setConfirmDelete(true)} />
            ))}
        </>
      }
    >
      <div className="sql-info env-page">
        <Toast kind={error ? "error" : "ok"} text={error ?? notice} onDismiss={() => { setError(null); setNotice(null); }} />
        <InfoTable>
          <InfoLine label="Name">
            <input value={draft.name} placeholder="e.g. Staging" onChange={(e) => set("name", e.target.value)} />
          </InfoLine>
          <InfoLine label="Run over ssh">
            <label className="check">
              <input type="checkbox" checked={draft.viaSsh} onChange={(e) => set("viaSsh", e.target.checked)} />
              <span>The query runs inside the container on the server</span>
            </label>
          </InfoLine>
          {draft.viaSsh ? (
            <>
              <InfoLine label="SSH host">
                <input value={draft.sshHost} placeholder="quizbuks-staging" onChange={(e) => set("sshHost", e.target.value)} />
              </InfoLine>
              <InfoLine label="Container">
                <input value={draft.container} placeholder="postgres" onChange={(e) => set("container", e.target.value)} />
              </InfoLine>
            </>
          ) : (
            <>
              <InfoLine label="Host">
                <input value={draft.host} placeholder="db.example.com" onChange={(e) => set("host", e.target.value)} />
              </InfoLine>
              <InfoLine label="Port">
                <input value={draft.port} onChange={(e) => set("port", e.target.value)} />
              </InfoLine>
            </>
          )}
          <InfoLine label="Database">
            <input value={draft.database} placeholder="quizbuks_staging" onChange={(e) => set("database", e.target.value)} />
          </InfoLine>
          <InfoLine label="User">
            <input value={draft.user} placeholder="postgres" onChange={(e) => set("user", e.target.value)} />
          </InfoLine>
          {!draft.viaSsh && (
            <InfoLine label="Password">
              <input
                type="password"
                value={password}
                placeholder={original?.password_saved ? "saved in the keychain (leave empty to keep)" : "not saved"}
                onChange={(e) => {
                  setPassword(e.target.value);
                  setClearPassword(false);
                }}
              />
            </InfoLine>
          )}
        </InfoTable>
        {original?.password_saved && !draft.viaSsh && (
          <label className="check">
            <input type="checkbox" checked={clearPassword} onChange={(e) => setClearPassword(e.target.checked)} />
            <span>Remove the saved password</span>
          </label>
        )}
      </div>
    </EditorPane>
  );
}


/**
 * The data source a statement or environment uses. "Environment or default" means none of its own:
 * the environment in use, or the default data source, decides.
 */
export function DataSourceSelect(props: {
  value: number | null;
  datasources: Datasource[];
  onChange: (value: number | null) => void;
}) {
  const current = props.datasources.find((d) => d.id === props.value);
  return (
    <PickMenu
      label={current ? `${current.name}${current.via_ssh ? " (over ssh)" : ""}` : "Environment or default"}
      items={[
        { key: "none", label: "Environment or default", active: props.value === null, onPick: () => props.onChange(null) },
        ...props.datasources.map((d) => ({
          key: String(d.id),
          label: `${d.name}${d.via_ssh ? " (over ssh)" : ""}`,
          active: props.value === d.id,
          onPick: () => props.onChange(d.id),
        })),
      ]}
    />
  );
}
