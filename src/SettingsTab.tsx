import { useEffect, useState } from "react";
import { invoke } from "./api";
import { open } from "@tauri-apps/plugin-dialog";
import { Section } from "./components";
import { Datasource, Settings } from "./types";

/** Catalog folder, mapping file and the default data source. Saved to localStorage for now. */
export function SettingsTab(props: {
  settings: Settings;
  setSettings: (s: Settings) => void;
  datasources: Datasource[];
  onImportConnection: () => void;
  knowledgeStatus: string | null;
  onIndexKnowledge: () => void;
}) {
  const { settings, setSettings, datasources, onImportConnection, knowledgeStatus, onIndexKnowledge } = props;
  const [aiKey, setAiKey] = useState("");
  const [aiKeySaved, setAiKeySaved] = useState(false);
  const [aiNote, setAiNote] = useState<string | null>(null);
  useEffect(() => {
    invoke<boolean>("ai_key_saved").then(setAiKeySaved).catch(() => setAiKeySaved(false));
  }, []);
  return (
    <div className="page settings-page">
      <Section title="Catalog" open>
        <label className="field">
          Catalog folder
          <input value={settings.catalogDir} onChange={(e) => setSettings({ ...settings, catalogDir: e.target.value })} />
          <small className="hint">Folder with the quizbuks .sql files. Reindex after it changes.</small>
        </label>
      </Section>
      <Section title="Mapping" open>
        <label className="field">
          Mapping JSON
          <input
            placeholder="/path/to/mapping.json"
            value={settings.mappingPath}
            onChange={(e) => setSettings({ ...settings, mappingPath: e.target.value })}
          />
          <small className="hint">Maps {"{{Entity.field}}"} placeholders to table and column names.</small>
        </label>
      </Section>
      <Section title="AI analysis" open>
        <label className="field">
          Anthropic API key
          <div className="dir-row">
            <input
              type="password"
              value={aiKey}
              placeholder={aiKeySaved ? "saved in the keychain (type a new key to replace it)" : "paste the key once"}
              onChange={(e) => setAiKey(e.target.value)}
            />
            <button
              className="small"
              disabled={!aiKey.trim()}
              onClick={async () => {
                try {
                  await invoke("save_ai_key", { key: aiKey });
                  setAiKey("");
                  setAiKeySaved(true);
                  setAiNote("Saved in the keychain.");
                } catch (e) {
                  setAiNote(String(e));
                }
              }}
            >
              Save key
            </button>
          </div>
          <small className="hint">
            {aiKeySaved ? "A key is saved in the keychain. " : "No key yet. "}
            {aiNote ?? ""}
          </small>
        </label>
        {aiKeySaved && (
          <button
            className="small ghost"
            onClick={async () => {
              await invoke("remove_ai_key");
              setAiKeySaved(false);
              setAiNote("Removed from the keychain.");
            }}
          >
            Remove the key
          </button>
        )}
        <label className="field">
          Model
          <select value={settings.aiModel} onChange={(e) => setSettings({ ...settings, aiModel: e.target.value })}>
            <option value="claude-sonnet-5-5">Claude Sonnet 5.5 (default)</option>
            <option value="claude-opus-5-5">Claude Opus 5.5</option>
            <option value="claude-haiku-5-5">Claude Haiku 5.5</option>
          </select>
        </label>
      </Section>
      <Section title="Knowledge" open>
        <label className="field">
          Knowledge folder
          <div className="dir-row">
            <input
              value={settings.knowledgeDir}
              placeholder="the folder with the system's Markdown documents"
              onChange={(e) => setSettings({ ...settings, knowledgeDir: e.target.value })}
            />
            <button className="small" onClick={() => onIndexKnowledge()}>
              Index now
            </button>
          </div>
          <small className="hint">
            The Markdown documents here are indexed, so an AI analysis can pick the ones about the tables it is looking at.
            {knowledgeStatus ? ` ${knowledgeStatus}` : ""}
          </small>
        </label>
      </Section>
      <Section title="Export" open>
        <label className="field">
          Export folder
          <div className="dir-row">
            <input
              placeholder="Not set: ask for a file each time"
              value={settings.exportDir}
              onChange={(e) => setSettings({ ...settings, exportDir: e.target.value })}
            />
            <button
              className="small"
              onClick={async () => {
                const dir = await open({ directory: true, multiple: false, defaultPath: settings.exportDir || undefined });
                if (typeof dir === "string") setSettings({ ...settings, exportDir: dir });
              }}
            >
              Choose…
            </button>
          </div>
          <small className="hint">
            CSV exports go here as "name time.csv", without asking. Clear it to be asked for a file each time.
          </small>
        </label>
      </Section>
      <Section title="Data source" open>
        <label className="field">
          Default data source
          <select
            value={settings.defaultDatasource ?? ""}
            onChange={(e) => setSettings({ ...settings, defaultDatasource: e.target.value === "" ? null : Number(e.target.value) })}
          >
            <option value="">None</option>
            {datasources.map((d) => (
              <option key={d.id} value={d.id}>
                {d.name}
                {d.via_ssh ? " (over ssh)" : ""}
              </option>
            ))}
          </select>
          <small className="hint">
            Statements and environments that choose none run on this one. Add and edit data sources in the Data
            sources panel.
          </small>
        </label>
        {settings.conn && (
          <div className="import-row">
            <span className="hint">A connection from before data sources existed is still stored here.</span>
            <button className="small" onClick={onImportConnection}>
              Import it as a data source
            </button>
          </div>
        )}
      </Section>
    </div>
  );
}
