import { useEffect, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { invoke, useBusy, usePending } from "./api";
import "./App.css";
import { BottomPanel } from "./BottomPanel";
import { CatalogEditor } from "./CatalogEditor";
import { CatalogSide, HistorySide, SavedSide } from "./SidePanels";
import { PickMenu, Spinner } from "./components";
import { Book, Database, Download, FileCode, Gear, History, Layers, List, Script, Sparkles, Table, Terminal, X } from "./icons";
import { CommandEditor, CommandSide, commandKey, SavedCommand } from "./ServerCommands";
import { LocalScript, LocalScriptEditor, LocalScriptSide, scriptKey } from "./LocalScripts";
import { DataSourceEditor, DataSourcesSide, dsKey } from "./DataSources";
import { SavedResultsSide } from "./ResultsSide";
import { SavedResultTab, resultKey } from "./SavedResultTab";
import { BackupTab, backupKey } from "./BackupTab";
import { AnalysisTab, AnalysisInput } from "./AnalysisTab";
import { GenerateDialog } from "./GenerateDialog";
import { TablesSide, TableTab, tableKey } from "./Tables";
import { SavedEditor } from "./SavedEditor";
import { EnvEditor, EnvPicker, EnvSide, envKey } from "./EnvEditor";
import { SettingsTab } from "./SettingsTab";
import { beginDrag, clamp, loadNumber, saveNumber, Splitter } from "./resize";
import {
  loadSettings,
  Output,
  QueryHit,
  Reuse,
  RunRecord,
  SavedSql,
  saveSettings,
  Settings,
  Tab,
  Environment,
  Accepted,
  GenerateRequest,
  Datasource,
  effectiveDatasource,
  QueryResult,
  effectiveValues,
} from "./types";

type Activity = "catalog" | "saved" | "env" | "ds" | "results" | "commands" | "scripts" | "tables" | "history";

const ACTIVITIES: { id: Activity; icon: ReactNode; label: string; title: string }[] = [
  { id: "catalog", icon: <Book size={22} />, label: "Catalog", title: "Catalog" },
  { id: "saved", icon: <FileCode size={22} />, label: "Saved SQL", title: "Saved SQL" },
  { id: "env", icon: <Layers size={22} />, label: "Environments", title: "Environments" },
  { id: "ds", icon: <Database size={22} />, label: "Data sources", title: "Data sources" },
  { id: "results", icon: <Download size={22} />, label: "Saved results", title: "Saved results" },
  { id: "commands", icon: <Terminal size={22} />, label: "Server commands", title: "Server commands" },
  { id: "scripts", icon: <Script size={22} />, label: "Local scripts", title: "Local scripts" },
  { id: "tables", icon: <Table size={22} />, label: "Tables", title: "Tables" },
  { id: "history", icon: <History size={22} />, label: "History", title: "History" },
];

const SIDE_MIN = 200;
const SIDE_MAX = 480;
const BOTTOM_MIN = 120;
const BOTTOM_MAX = 600;

/**
 * VS Code layout in Vioros colours: an activity bar, a side panel with the groups as trees,
 * editor tabs for the open queries and statements, and a bottom panel with results and runs.
 */
export default function App() {
  const [settings, setSettings] = useState<Settings>(loadSettings);
  const [activity, setActivity] = useState<Activity>("catalog");
  const [filter, setFilter] = useState("");
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [active, setActive] = useState<string | null>(null);
  const [outputs, setOutputs] = useState<Record<string, Output>>({});
  const [tabMenu, setTabMenu] = useState(false);
  const [analysisInputs, setAnalysisInputs] = useState<Record<string, AnalysisInput>>({});
  const [generateReqs, setGenerateReqs] = useState<Record<string, GenerateRequest>>({});
  const [accepted, setAccepted] = useState<Accepted | null>(null);
  const [envs, setEnvs] = useState<Environment[]>([]);
  const [reuse, setReuse] = useState<Reuse | null>(null);
  const [version, setVersion] = useState(0);
  const [notice, setNotice] = useState<string | null>(null);
  const [cursor, setCursor] = useState<{ line: number; column: number } | null>(null);
  const pending = usePending();
  const busy = useBusy();
  const [sideW, setSideW] = useState(() => loadNumber("side-width.v1", 260, SIDE_MIN, SIDE_MAX));
  const [bottomH, setBottomH] = useState(() => loadNumber("bottom-h.v1", 240, BOTTOM_MIN, BOTTOM_MAX));

  useEffect(() => saveSettings(settings), [settings]);

  // The environments, reloaded whenever something changed (the settings tab edits them).
  useEffect(() => {
    invoke<Environment[]>("list_environments")
      .then(setEnvs)
      .catch(() => setEnvs([]));
  }, [version]);
  const activeEnv = effectiveValues(envs, settings.activeEnv);
  const envDs = effectiveDatasource(envs, settings.activeEnv);

  // The data sources, reloaded whenever something changed (the data source editor and the settings edit them).
  const [datasources, setDatasources] = useState<Datasource[]>([]);
  useEffect(() => {
    invoke<Datasource[]>("list_datasources")
      .then(setDatasources)
      .catch(() => setDatasources([]));
  }, [version]);
  const defaultDs = settings.defaultDatasource;

  // The knowledge folder is indexed on start, and again whenever "Index now" is pressed.
  const [knowledgeStatus, setKnowledgeStatus] = useState<string | null>(null);
  async function indexKnowledge() {
    if (!settings.knowledgeDir.trim()) return setKnowledgeStatus("No knowledge folder set.");
    setKnowledgeStatus("Indexing…");
    try {
      const r = await invoke<{ indexed: number; unchanged: number; removed: number; skipped: number; total_docs: number; total_bytes: number }>(
        "index_knowledge",
        { dir: settings.knowledgeDir.trim() },
      );
      setKnowledgeStatus(
        `${r.total_docs} documents, ${Math.round(r.total_bytes / 1024)} KB indexed (${r.indexed} read now, ${r.removed} removed, ${r.skipped} skipped).`,
      );
    } catch (e) {
      setKnowledgeStatus(String(e));
    }
  }
  useEffect(() => {
    if (settings.knowledgeDir.trim()) indexKnowledge();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Imports the connection used before data sources existed, as the default data source. */
  async function importConnection() {
    try {
      const id = await invoke<number>("import_connection", { name: "Imported", url: settings.conn });
      setSettings({ ...settings, conn: "", defaultDatasource: id });
      setNotice(null);
      bump();
    } catch (e) {
      setNotice(String(e));
    }
  }

  /** Opens a kept result in its own tab. */
  function openSavedResult(id: number, name: string) {
    openTab({ key: resultKey(id), kind: "result", title: name, queryId: "" });
  }

  // Cmd/Ctrl+K focuses the search; the Settings menu item opens the settings tab.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        document.querySelector<HTMLInputElement>(".side-search input")?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    const unlisten = listen("open-settings", () => openSettings());
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tabs, active]);

  const activeTab = tabs.find((t) => t.key === active) ?? null;
  const bump = () => setVersion((v) => v + 1);

  function openTab(t: Tab) {
    setTabs((prev) => (prev.some((x) => x.key === t.key) ? prev : [...prev, t]));
    setActive(t.key);
  }

  function closeTab(key: string) {
    const i = tabs.findIndex((t) => t.key === key);
    if (i < 0) return;
    const next = tabs.filter((t) => t.key !== key);
    setTabs(next);
    if (active === key) setActive(next[i]?.key ?? next[i - 1]?.key ?? null);
  }

  /** Closes every open tab (the outputs stay in the bottom panel's history). */
  function closeAllTabs() {
    setTabs([]);
    setActive(null);
    setTabMenu(false);
  }

  /** A draft tab became a saved statement. */
  function replaceTab(oldKey: string, t: Tab) {
    setTabs((prev) => prev.map((x) => (x.key === oldKey ? t : x)));
    if (active === oldKey) setActive(t.key);
    setOutputs((prev) => {
      const out = prev[oldKey];
      if (!out) return prev;
      const next = { ...prev, [t.key]: out };
      delete next[oldKey];
      return next;
    });
  }

  function setOutput(key: string, out: Output) {
    setOutputs((prev) => ({ ...prev, [key]: out }));
  }

  function openSettings() {
    openTab({ key: "settings", kind: "settings", title: "Settings", queryId: "" });
  }

  function openDs(id: number) {
    const d = datasources.find((x) => x.id === id);
    openTab({ key: dsKey(id), kind: "ds", title: d?.name ?? "Data source", queryId: "" });
  }

  /** Opens the Generate tab for a statement or command; an accepted proposal comes back to its editor. */
  function openGenerate(req: GenerateRequest) {
    const key = `generate:${Date.now()}`;
    setGenerateReqs((prev) => ({ ...prev, [key]: req }));
    openTab({ key, kind: "generate", title: req.kind === "sql" ? "Generate SQL" : req.kind === "script" ? "Generate script" : "Generate command", queryId: "" });
  }

  /** Opens the AI analysis of a result in its own tab; the result is what the analysis is about. */
  async function analyze(result: QueryResult, from: Tab, datasourceId: number | null) {
    // An analysis belongs to a saved result: a result that is not kept yet is kept first.
    let resultId: number | null = from.key.startsWith("result:") ? Number(from.key.slice("result:".length)) : null;
    if (resultId === null) {
      try {
        resultId = await invoke<number>("save_result", {
          args: {
            name: `${from.title} · ${new Date().toLocaleString()}`,
            source: from.queryId || from.title,
            datasourceId,
            sql: result.executed_sql,
            result: { columns: result.columns, rows: result.rows, truncated: result.truncated },
          },
        });
        bump();
      } catch (e) {
        setNotice(String(e));
        return;
      }
    }
    const key = `analysis:${Date.now()}`;
    setAnalysisInputs((prev) => ({
      ...prev,
      [key]: {
        executed: result.executed_sql,
        description: from.title,
        columns: result.columns,
        rows: result.rows,
        totalRows: result.rows.length,
        truncated: result.truncated,
        resultId,
      },
    }));
    openTab({ key, kind: "analysis", title: `Analysis: ${from.title}`, queryId: "" });
  }

  /** Opens a saved analysis (its name, request and answer) in its own tab. */
  function openSavedAnalysis(id: number, name: string) {
    openTab({ key: `analysis:saved-${id}`, kind: "analysis", title: name, queryId: "" });
  }

  function openBackup() {
    openTab({ key: backupKey(), kind: "backup", title: "Back up a server database", queryId: "" });
  }

  function newDs() {
    openTab({ key: `ds:new-${Date.now()}`, kind: "ds", title: "New data source", queryId: "" });
  }

  function openEnv(name: string) {
    openTab({ key: envKey(name), kind: "env", title: name, queryId: "" });
  }

  /** A new environment: an editor tab with an empty name, saved from its toolbar. */
  function newEnv(parent: string | null) {
    openTab({ key: `env:new-${Date.now()}`, kind: "env", title: "New environment", queryId: "", group: parent ?? "" });
  }

  /** Moves an environment under another one (or to the top) in the side tree, keeping its values. */
  async function moveEnv(name: string, parent: string | null) {
    const env = envs.find((e) => e.name === name);
    if (!env) return;
    try {
      await invoke("save_environment", {
        name,
        parent,
        description: env.description,
        values: env.values,
        datasourceId: env.datasource_id,
      });
      setNotice(null);
      bump();
    } catch (e) {
      setNotice(String(e));
    }
  }

  function openCatalog(hit: QueryHit) {
    openTab({ key: `catalog:${hit.path}`, kind: "catalog", title: hit.query_id, queryId: hit.query_id, path: hit.path });
  }

  function openSaved(item: SavedSql) {
    openTab({ key: `saved:${item.id}`, kind: "saved", title: item.name, queryId: `saved:${item.id}`, id: item.id });
  }

  /** A new statement, filed in `group` ("" for the top). */
  function newStatement(group: string) {
    openTab({ key: `draft:${Date.now()}`, kind: "saved", title: "New statement", queryId: "", id: null, group });
  }

  /** A run in History: open its query (or statement) with the values it ran with. */
  async function openRun(run: RunRecord) {
    if (run.query_id.startsWith("saved:")) {
      const id = Number(run.query_id.slice(6));
      const t: Tab = { key: `saved:${id}`, kind: "saved", title: `Saved statement #${id}`, queryId: run.query_id, id };
      openTab(t);
      setReuse({ key: t.key, params: run.params_json, nonce: Date.now() });
      return;
    }
    try {
      // Look the query up by id. (A text search cannot find the id: it is not in the search index.)
      const hit = await invoke<QueryHit | null>("find_query", { queryId: run.query_id });
      if (!hit) {
        setNotice(`${run.query_id} is not in the catalog any more. Reindex, or check the catalog folder.`);
        return;
      }
      openCatalog(hit);
      setReuse({ key: `catalog:${hit.path}`, params: run.params_json, nonce: Date.now() });
    } catch (e) {
      setNotice(String(e));
    }
  }

  /** Save as from a catalog query or a statement: open the new statement with the values filled in. */
  function openSavedAs(t: Tab, params: Record<string, string | null>) {
    openTab(t);
    setReuse({ key: t.key, params: JSON.stringify(params), nonce: Date.now() });
    bump();
  }

  function reuseInActive(run: RunRecord) {
    if (!active) return;
    setReuse({ key: active, params: run.params_json, nonce: Date.now() });
  }

  function startSideDrag(e: ReactPointerEvent) {
    const start = sideW;
    let latest = start;
    beginDrag(
      e,
      "x",
      (d) => {
        latest = clamp(start + d, SIDE_MIN, SIDE_MAX);
        setSideW(latest);
      },
      () => saveNumber("side-width.v1", latest),
    );
  }

  function startBottomDrag(e: ReactPointerEvent) {
    const start = bottomH;
    let latest = start;
    beginDrag(
      e,
      "y",
      (d) => {
        latest = clamp(start - d, BOTTOM_MIN, BOTTOM_MAX);
        setBottomH(latest);
      },
      () => saveNumber("bottom-h.v1", latest),
    );
  }

  function renderEditor(t: Tab) {
    if (t.kind === "catalog") {
      return (
        <CatalogEditor
          key={t.key}
          tab={t}
          settings={settings}
          reuse={reuse}
          onOutput={setOutput}
          onChanged={bump}
          onCursor={(line, column) => setCursor({ line, column })}
          onSavedAs={openSavedAs}
          env={activeEnv}
          datasources={datasources}
          defaultDs={defaultDs}
          envDs={envDs}
        />
      );
    }
    if (t.kind === "saved") {
      return (
        <SavedEditor
          key={t.key}
          tab={t}
          settings={settings}
          reuse={reuse}
          onOutput={setOutput}
          onChanged={bump}
          onSaved={(nt) => replaceTab(t.key, nt)}
          onDeleted={(k) => closeTab(k)}
          onCursor={(line, column) => setCursor({ line, column })}
          onCopied={openSavedAs}
          env={activeEnv}
          onGenerate={openGenerate}
          accepted={accepted}
          datasources={datasources}
          defaultDs={defaultDs}
          envDs={envDs}
        />
      );
    }
    if (t.kind === "env") {
      return (
        <EnvEditor
          key={t.key}
          tab={t}
          envs={envs}
          datasources={datasources}
          activeEnv={settings.activeEnv}
          onUse={(name) => setSettings({ ...settings, activeEnv: name })}
          onChanged={bump}
          onRenamed={(oldKey, nt) => replaceTab(oldKey, nt)}
          onDeleted={(k) => closeTab(k)}
        />
      );
    }
    if (t.kind === "result") {
      return (
        <SavedResultTab
          key={t.key}
          tab={t}
          datasources={datasources}
          exportDir={settings.exportDir}
          onExportDir={(dir) => setSettings({ ...settings, exportDir: dir })}
          onChanged={bump}
          onDeleted={(k) => closeTab(k)}
          onOutput={setOutput}
        />
      );
    }
    if (t.kind === "table") {
      return (
        <TableTab
          key={t.key}
          tab={t}
          mappingPath={settings.mappingPath}
          defaultDs={settings.defaultDatasource}
          onOutput={setOutput}
        />
      );
    }
    if (t.kind === "generate") {
      const req = generateReqs[t.key];
      if (!req) return <div className="empty">This generation is no longer here.</div>;
      return (
        <GenerateDialog
          key={t.key}
          kind={req.kind}
          description={req.description}
          current={req.current}
          server={req.server}
          mappingPath={req.mappingPath}
          model={settings.aiModel}
          onAccept={(text) => {
            setAccepted({ key: req.originKey, text, nonce: Date.now() });
            closeTab(t.key);
            setActive(req.originKey);
          }}
          onClose={() => closeTab(t.key)}
        />
      );
    }
    if (t.kind === "analysis") {
      return (
        <AnalysisTab
          key={t.key}
          tab={t}
          input={analysisInputs[t.key]}
          model={settings.aiModel}
          onSaved={(nt) => replaceTab(t.key, nt)}
          onDeleted={(k) => closeTab(k)}
          onChanged={bump}
        />
      );
    }
    if (t.kind === "command") {
      return (
        <CommandEditor
          key={t.key}
          tab={t}
          model={settings.aiModel}
          onGenerate={openGenerate}
          accepted={accepted}
          onOutput={setOutput}
          onChanged={bump}
          onSaved={(nt) => replaceTab(t.key, nt)}
          onDeleted={(k) => closeTab(k)}
          onCursor={(line, column) => setCursor({ line, column })}
        />
      );
    }
    if (t.kind === "script") {
      return (
        <LocalScriptEditor
          key={t.key}
          tab={t}
          env={activeEnv}
          onGenerate={openGenerate}
          accepted={accepted}
          onOutput={setOutput}
          onChanged={bump}
          onSaved={(nt) => replaceTab(t.key, nt)}
          onDeleted={(k) => closeTab(k)}
          onCursor={(line, column) => setCursor({ line, column })}
        />
      );
    }
    if (t.kind === "backup") {
      return <BackupTab key={t.key} tab={t} onChanged={bump} />;
    }
    if (t.kind === "ds") {
      return (
        <DataSourceEditor
          key={t.key}
          tab={t}
          datasources={datasources}
          defaultDs={defaultDs}
          onChanged={bump}
          onRenamed={(oldKey, nt) => replaceTab(oldKey, nt)}
          onDeleted={(k) => closeTab(k)}
          onDefault={(id) => setSettings({ ...settings, defaultDatasource: id })}
        />
      );
    }
    return (
      <SettingsTab
        settings={settings}
        setSettings={setSettings}
        datasources={datasources}
        onImportConnection={importConnection}
        knowledgeStatus={knowledgeStatus}
        onIndexKnowledge={indexKnowledge}
      />
    );
  }

  const defaultRow = datasources.find((d) => d.id === defaultDs) ?? null;
  const connText = defaultRow
    ? `Data source: ${defaultRow.name}${defaultRow.via_ssh ? " (over ssh)" : ""}`
    : datasources.length
      ? "No default data source. Choose one in Settings."
      : "No data source yet. Add one in Data sources.";
  const current = ACTIVITIES.find((a) => a.id === activity);

  return (
    <div className="app">
      <header className="top-bar">
        {/* the title sits over the activity bar and side panel; the environment starts where the editor starts */}
        <div className="brand-slot" style={{ marginLeft: 34, width: sideW + 8 }}>
          <div className="brand">sqlpool</div>
        </div>
        <EnvPicker envs={envs} value={settings.activeEnv} onChange={(name) => setSettings({ ...settings, activeEnv: name })} />
        {/* the default data source, on the right of the same bar; its place stays put when the environment changes */}
        <div className="top-default">
          <span className="meta">Default data source:</span>
          <div className="top-default-pick">
            <PickMenu
              label={defaultRow ? `${defaultRow.name}${defaultRow.via_ssh ? " (over ssh)" : ""}` : "None"}
              items={[
                {
                  key: "none",
                  label: "None",
                  active: defaultDs === null,
                  onPick: () => setSettings({ ...settings, defaultDatasource: null }),
                },
                ...datasources.map((d) => ({
                  key: String(d.id),
                  label: `${d.name}${d.via_ssh ? " (over ssh)" : ""}`,
                  active: defaultDs === d.id,
                  onPick: () => setSettings({ ...settings, defaultDatasource: d.id }),
                })),
              ]}
            />
          </div>
        </div>
      </header>

      <div className="workbench">
        <nav className="activity-bar">
          {ACTIVITIES.map((a) => (
            <button
              key={a.id}
              className={(activity === a.id ? "act active" : "act") + ` act-${a.id}`}
              title={a.label}
              onClick={() => setActivity(a.id)}
            >
              {a.icon}
            </button>
          ))}
          <span className="spacer" />
          <button className={active === "settings" ? "act active" : "act"} title="Settings" onClick={openSettings}>
            <Gear size={22} />
          </button>
        </nav>

        <aside className="side-panel" style={{ width: sideW }}>
          <div className="side-head">{current?.title.toUpperCase()}</div>
          <div className="side-body" hidden={activity !== "catalog"}>
            <CatalogSide
              filter={filter}
              onFilter={setFilter}
              version={version}
              activeKey={active}
              settingsDir={settings.catalogDir}
              onOpen={openCatalog}
              onReindexed={(n, skipped) =>
                setNotice(`Indexed ${n} queries${skipped.length ? `, ${skipped.length} skipped` : ""}.`)
              }
            />
          </div>
          <div className="side-body" hidden={activity !== "saved"}>
            <SavedSide
              filter={filter}
              onFilter={setFilter}
              version={version}
              activeKey={active}
              onOpen={openSaved}
              onNewStatement={newStatement}
              onChanged={bump}
            />
          </div>
          <div className="side-body" hidden={activity !== "env"}>
            <EnvSide
              envs={envs}
              filter={filter}
              onFilter={setFilter}
              activeKey={active}
              activeEnv={settings.activeEnv}
              onOpen={openEnv}
              onNew={newEnv}
              onMove={moveEnv}
            />
          </div>
          <div className="side-body" hidden={activity !== "ds"}>
            <DataSourcesSide
              datasources={datasources}
              filter={filter}
              onFilter={setFilter}
              activeKey={active}
              defaultDs={defaultDs}
              onOpen={openDs}
              onNew={newDs}
              onBackup={openBackup}
            />
          </div>
          <div className="side-body" hidden={activity !== "results"}>
            <SavedResultsSide
              filter={filter}
              onFilter={setFilter}
              version={version}
              onOpen={openSavedResult}
              onOpenAnalysis={openSavedAnalysis}
              activeKey={active}
            />
          </div>
          <div className="side-body" hidden={activity !== "commands"}>
            <CommandSide
              filter={filter}
              onFilter={setFilter}
              version={version}
              activeKey={active}
              onOpen={(c: SavedCommand) =>
                openTab({ key: commandKey(c.id), kind: "command", title: c.name, queryId: `command:${c.id}`, id: c.id, group: c.group })
              }
              onNew={(group) =>
                openTab({ key: `command:new-${Date.now()}`, kind: "command", title: "New command", queryId: "", group })
              }
              onChanged={bump}
            />
          </div>
          <div className="side-body" hidden={activity !== "scripts"}>
            <LocalScriptSide
              filter={filter}
              onFilter={setFilter}
              version={version}
              activeKey={active}
              onOpen={(s: LocalScript) =>
                openTab({ key: scriptKey(s.id), kind: "script", title: s.name, queryId: `script:${s.id}`, id: s.id, group: s.group })
              }
              onNew={(group) =>
                openTab({ key: `script:new-${Date.now()}`, kind: "script", title: "New script", queryId: "", group })
              }
              onChanged={bump}
            />
          </div>
          <div className="side-body" hidden={activity !== "tables"}>
            <TablesSide
              defaultDs={settings.defaultDatasource}
              mappingPath={settings.mappingPath}
              filter={filter}
              onFilter={setFilter}
              activeKey={active}
              onOpen={(table) => openTab({ key: tableKey(table), kind: "table", title: table, queryId: "" })}
              version={version}
            />
          </div>
          <div className="side-body" hidden={activity !== "history"}>
            <HistorySide filter={filter} onFilter={setFilter} version={version} onOpen={openRun} />
          </div>
        </aside>

        <Splitter axis="x" onPointerDown={startSideDrag} />

        <section className="editor-col">
          <div className="tab-strip-wrap">
          <div className="tab-strip">
            {tabs.map((t) => (
              <div
                key={t.key}
                className={t.key === active ? "tab-item active" : "tab-item"}
                onClick={() => setActive(t.key)}
              >
                <TabIcon kind={t.kind} />
                <span className="tab-title">{t.title}</span>
                <button
                  className="tab-close"
                  title="Close"
                  onClick={(e) => {
                    e.stopPropagation();
                    closeTab(t.key);
                  }}
                >
                  ×
                </button>
              </div>
            ))}
          </div>
          <button className="tab-all" title="All tabs" aria-label="All tabs" onClick={() => setTabMenu((m) => !m)}>
            <List size={16} />
          </button>
          {tabMenu && (
            <>
              <div className="tab-menu-backdrop" onClick={() => setTabMenu(false)} />
              <div className="tab-menu" role="menu">
                {tabs.length === 0 && <div className="tab-menu-empty">No open tabs</div>}
                {tabs.length > 0 && (
                  <table className="tab-menu-table">
                    <tbody>
                      {tabs.map((t) => (
                        <tr
                          key={t.key}
                          role="menuitem"
                          className={t.key === active ? "active" : undefined}
                          onClick={() => {
                            setActive(t.key);
                            setTabMenu(false);
                          }}
                        >
                          <td className="tab-menu-kind">
                            <TabIcon kind={t.kind} />
                          </td>
                          <td className="tab-menu-title">{t.title}</td>
                          <td className="tab-menu-close">
                            <button
                              className="tab-close"
                              title="Close"
                              onClick={(e) => {
                                e.stopPropagation();
                                closeTab(t.key);
                              }}
                            >
                              <X size={14} />
                            </button>
                          </td>
                        </tr>
                      ))}
                    </tbody>
                  </table>
                )}
                {tabs.length > 0 && (
                  <button className="tab-menu-close-all" onClick={closeAllTabs}>
                    Close all tabs
                  </button>
                )}
              </div>
            </>
          )}
          </div>

          <div className="editor-area">
            {tabs.length === 0 && <div className="empty">Open a query or statement from the side panel.</div>}
            {/* Every open tab stays mounted (hidden when not active), so switching is instant. */}
            {tabs.map((t) => (
              <div key={t.key} className="editor-slot" hidden={t.key !== active}>
                {renderEditor(t)}
              </div>
            ))}
          </div>

        </section>
      </div>

      {/* The results run the full width of the window along the bottom, like Vioros's Tasks bar. */}
      <Splitter axis="y" onPointerDown={startBottomDrag} />
      <div className="bottom-wrap" style={{ height: bottomH }}>
        <BottomPanel
          busy={busy}
          tab={activeTab}
          output={active ? outputs[active] : undefined}
          version={version}
          onReuse={reuseInActive}
          onKept={bump}
          onAnalyze={analyze}
          exportDir={settings.exportDir}
          onExportDir={(dir) => setSettings({ ...settings, exportDir: dir })}
        />
      </div>

      <footer className="status-bar">
        <span className={defaultRow ? "status-dot on" : "status-dot"} />
        <span>{connText}</span>
        {notice && <span className="status-notice">{notice}</span>}
        {pending > 0 && <Spinner label={pending > 1 ? `Working… (${pending})` : "Working…"} />}
        <span className="spacer" />
        {activeTab && (activeTab.kind === "saved" || activeTab.kind === "catalog") && cursor && (
          <span>
            Ln {cursor.line}, Col {cursor.column}
          </span>
        )}
        {active && outputs[active]?.result && (
          <span>
            {outputs[active].result?.rows.length} rows · {outputs[active].result?.elapsed_ms} ms
          </span>
        )}
        {active && outputs[active]?.error && <span className="status-error">Error</span>}
        <span>SQL</span>
        <span>Read-only</span>
      </footer>
    </div>
  );
}

/** A small icon before a tab's name, so the kinds of tab tell apart at a glance. */
function TabIcon(props: { kind: Tab["kind"] }) {
  const size = 18;
  const icon =
    props.kind === "catalog" ? (
      <Book size={size}  />
    ) : props.kind === "saved" ? (
      <FileCode size={size}  />
    ) : props.kind === "env" ? (
      <Layers size={size}  />
    ) : props.kind === "ds" ? (
      <Database size={size} />
    ) : props.kind === "result" || props.kind === "backup" ? (
      <Download size={size} />
    ) : props.kind === "command" || props.kind === "script" ? (
      <Terminal size={size} />
    ) : props.kind === "analysis" || props.kind === "generate" ? (
      <Sparkles size={size} />
    ) : props.kind === "table" ? (
      <Table size={size} />
    ) : (
      <Gear size={size}  />
    );
  return <span className={`tab-icon kind-${props.kind}`} aria-hidden="true">{icon}</span>;
}
