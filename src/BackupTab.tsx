import { useState } from "react";
import { invoke } from "./api";
import { Toast } from "./components";
import { EditorPane, InfoLine, InfoTable, ToolButton } from "./EditorPane";
import { Tab } from "./types";

/** The key for a backup tab. */
export function backupKey(): string {
  return `backup:new-${Date.now()}`;
}

/** The local date as yyyymmdd, for the default name of the restored database. */
function yyyymmdd(d = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
}

type Form = {
  sshHost: string;
  remoteDir: string;
  remoteContainer: string;
  remoteDb: string;
  localContainer: string;
  localPort: string;
  localUser: string;
  localPassword: string;
  localDb: string;
  datasourceName: string;
};

function initialForm(): Form {
  const remoteDb = "quizbuks_staging";
  return {
    sshHost: "quizbuks-staging",
    remoteDir: "~",
    remoteContainer: "postgres",
    remoteDb,
    localContainer: "quizbuks-staging",
    localPort: "55433",
    localUser: "postgres",
    localPassword: "",
    localDb: `${remoteDb}_${yyyymmdd()}`,
    datasourceName: `Backup ${remoteDb} ${yyyymmdd()}`,
  };
}

/**
 * One button: dump the server's database over ssh (the server's own pg_dump), download the dump,
 * restore it into a new database in a local Docker container, and create a data source for it.
 */
export function BackupTab(props: { tab: Tab; onChanged: () => void }) {
  const { onChanged } = props;
  const [form, setForm] = useState<Form>(initialForm);
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [warnings, setWarnings] = useState<string>("");

  function set<K extends keyof Form>(k: K, v: Form[K]) {
    setForm((f) => ({ ...f, [k]: v }));
  }

  async function run() {
    if (running) return;
    setRunning(true);
    setError(null);
    setNotice(null);
    setWarnings("");
    try {
      const r = await invoke<{ dumpPath: string; bytes: number; localDb: string; datasourceId: number; warnings: string }>(
        "back_up_database",
        {
          args: {
            sshHost: form.sshHost.trim(),
            remoteDir: form.remoteDir.trim(),
            remoteContainer: form.remoteContainer.trim(),
            remoteDb: form.remoteDb.trim(),
            localContainer: form.localContainer.trim(),
            localDb: form.localDb.trim(),
            datasourceName: form.datasourceName.trim(),
            localPort: Number(form.localPort) || 5432,
            localUser: form.localUser.trim(),
            localPassword: form.localPassword,
          },
        },
      );
      setNotice(`Restored into ${r.localDb} (${Math.round(r.bytes / 1024)} KB). Data source "${form.datasourceName.trim()}" created.`);
      setWarnings(r.warnings);
      setForm((f) => ({ ...f, localPassword: "" }));
      onChanged();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  return (
    <EditorPane
      crumbs={["Data sources", "Back up a server database"]}
      title="Back up a server database"
      actions={
        <ToolButton
          icon="download"
          label={running ? "Backing up…" : "Back up and restore"}
          primary
          busy={running}
          disabled={running}
          onClick={run}
          title="Dump the server's database, download it, restore it locally and create a data source"
        />
      }
    >
      <div className="sql-info env-page">
        <Toast kind={error ? "error" : "ok"} text={error ?? notice} onDismiss={() => { setError(null); setNotice(null); }} />
        <InfoTable>
          <InfoLine label="Server (ssh)">
            <input value={form.sshHost} onChange={(e) => set("sshHost", e.target.value)} />
          </InfoLine>
          <InfoLine label="Dump folder on server">
            <input value={form.remoteDir} onChange={(e) => set("remoteDir", e.target.value)} />
          </InfoLine>
          <InfoLine label="Server container">
            <input value={form.remoteContainer} onChange={(e) => set("remoteContainer", e.target.value)} />
          </InfoLine>
          <InfoLine label="Server database">
            <input
              value={form.remoteDb}
              onChange={(e) => {
                const db = e.target.value;
                setForm((f) => ({
                  ...f,
                  remoteDb: db,
                  localDb: `${db}_${yyyymmdd()}`,
                  datasourceName: `Backup ${db} ${yyyymmdd()}`,
                }));
              }}
            />
          </InfoLine>
          <InfoLine label="Local container">
            <input value={form.localContainer} onChange={(e) => set("localContainer", e.target.value)} />
          </InfoLine>
          <InfoLine label="Local port">
            <input value={form.localPort} onChange={(e) => set("localPort", e.target.value)} />
          </InfoLine>
          <InfoLine label="Local user">
            <input value={form.localUser} onChange={(e) => set("localUser", e.target.value)} />
          </InfoLine>
          <InfoLine label="Local password">
            <input
              type="password"
              value={form.localPassword}
              placeholder="saved in the keychain for the new data source"
              onChange={(e) => set("localPassword", e.target.value)}
            />
          </InfoLine>
          <InfoLine label="New database">
            <input value={form.localDb} onChange={(e) => set("localDb", e.target.value)} />
          </InfoLine>
          <InfoLine label="Data source name">
            <input value={form.datasourceName} onChange={(e) => set("datasourceName", e.target.value)} />
          </InfoLine>
        </InfoTable>
        {running && <div className="meta small">Dumping on the server, downloading, and restoring. This can take a few minutes.</div>}
        {warnings && (
          <>
            <div className="info-title">pg_restore reported</div>
            <pre className="sent-sql">{warnings}</pre>
          </>
        )}
      </div>
    </EditorPane>
  );
}
