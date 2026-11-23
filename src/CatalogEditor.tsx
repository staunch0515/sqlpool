import { useEffect, useState } from "react";
import { invoke } from "./api";
import { Banner, BusyOverlay } from "./components";
import { InfoLine, InfoTable, SqlPage, ToolButton, ToolSep } from "./EditorPane";
import { groupOf } from "./SidePanels";
import { DataSourceSelect } from "./DataSources";
import { SqlEditor } from "./SqlEditor";
import { TimeInput } from "./TimeInput";
import {
  Datasource,
  resolveDatasource,
  asOfFromRun,
  asOfSql,
  controlFor,
  fromSqlValue,
  Output,
  parseParams,
  QueryDetail,
  QueryHit,
  QueryResult,
  Reuse,
  Settings,
  Tab,
  toSqlValue,
} from "./types";

/**
 * Editor tab for a catalog query. Layout: breadcrumb, toolbar (run, favourite, copy),
 * the parameter strip, then the SQL (read-only). Results go to the bottom panel.
 */
export function CatalogEditor(props: {
  tab: Tab;
  settings: Settings;
  reuse: Reuse | null;
  onOutput: (key: string, out: Output) => void;
  onChanged: () => void;
  onCursor: (line: number, column: number) => void;
  /** "Save as": the new saved statement, and the parameter values to fill it with. */
  onSavedAs: (tab: Tab, params: Record<string, string | null>) => void;
  /** The active environment's values: they fill the parameters first. */
  env: Record<string, string>;
  datasources: Datasource[];
  defaultDs: number | null;
  envDs: number | null;
}) {
  const { tab, settings, reuse, onOutput, onChanged, onCursor, onSavedAs, env, datasources, defaultDs, envDs } = props;
  const [dsChoice, setDsChoice] = useState<number | null>(null);
  const resolvedDs = resolveDatasource(dsChoice, envDs, defaultDs);

  // The data source chosen for this query, if any.
  useEffect(() => {
    invoke<number | null>("query_datasource", { queryId: tab.queryId })
      .then(setDsChoice)
      .catch(() => setDsChoice(null));
  }, [tab.queryId]);

  function chooseDs(v: number | null) {
    setDsChoice(v);
    invoke("set_query_datasource", { queryId: tab.queryId, datasourceId: v }).catch((e) =>
      onOutput(tab.key, { result: null, error: String(e) }),
    );
  }
  const [saving, setSaving] = useState(false);
  const [hit, setHit] = useState<QueryHit | null>(null);
  const [detail, setDetail] = useState<QueryDetail | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [asOf, setAsOf] = useState("");
  const [running, setRunning] = useState(false);

  // Load the query and its index row (favourite, tags).
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const d = await invoke<QueryDetail>("get_query", {
          path: tab.path ?? "",
          mappingPath: settings.mappingPath || null,
        });
        const h = await invoke<QueryHit | null>("find_query", { queryId: tab.queryId });
        if (cancelled) return;
        const empty: Record<string, string> = {};
        for (const name of Object.keys(d.front.params)) empty[name] = "";
        setDetail(d);
        setHit(h);
        setValues(empty);
        setAsOf("");
        setLoadError(null);
      } catch (e) {
        if (!cancelled) setLoadError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tab.key, settings.mappingPath]);

  // Parameters that the active environment defines take its values (when loaded, or when the environment changes).
  const envJson = JSON.stringify(env);
  useEffect(() => {
    if (!detail) return;
    const fromEnv = JSON.parse(envJson) as Record<string, string>;
    setValues((prev) => {
      const next = { ...prev };
      for (const name of Object.keys(detail.front.params)) {
        if (name in fromEnv) next[name] = fromEnv[name];
      }
      return next;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail, envJson]);

  // "Open" from the run history: restore the values the run used.
  useEffect(() => {
    if (!reuse || reuse.key !== tab.key || !detail) return;
    const saved = parseParams(reuse.params);
    const next: Record<string, string> = {};
    for (const [name, spec] of Object.entries(detail.front.params)) {
      next[name] = fromSqlValue(spec, saved[name] ?? null);
    }
    setValues(next);
    setAsOf(asOfFromRun(saved));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [reuse?.nonce, detail]);

  async function run() {
    if (!detail || running) return;
    setRunning(true);
    const params: Record<string, string | null> = {};
    for (const name of Object.keys(detail.front.params)) {
      const v = values[name]?.trim() ?? "";
      params[name] = v === "" ? null : toSqlValue(detail.front.params[name], v);
    }
    try {
      if (resolvedDs === null) throw new Error("No data source: choose one here, in the environment, or as the default.");
      const result = await invoke<QueryResult>("run_query", {
        args: {
          path: tab.path,
          mappingPath: settings.mappingPath || null,
          datasourceId: resolvedDs,
          params,
          asOf: asOfSql(asOf),
        },
      });
      onOutput(tab.key, { result, error: null, source: tab.queryId, datasourceId: resolvedDs });
    } catch (e) {
      onOutput(tab.key, { result: null, error: String(e) });
    } finally {
      setRunning(false);
      onChanged();
    }
  }

  /** Builds the parameter values the way a run does (SQL text, null for empty). */
  function paramValues(): Record<string, string | null> {
    const out: Record<string, string | null> = {};
    if (!detail) return out;
    for (const name of Object.keys(detail.front.params)) {
      const v = values[name]?.trim() ?? "";
      out[name] = v === "" ? null : toSqlValue(detail.front.params[name], v);
    }
    return out;
  }

  /** Stores this query as a new saved statement, parameters included, and opens it. */
  async function saveAs() {
    if (!detail || saving) return;
    setSaving(true);
    const params = paramValues();
    try {
      const id = await invoke<number>("save_catalog_as", {
        args: {
          path: tab.path,
          mappingPath: settings.mappingPath || null,
          name: detail.front.id,
          group: "",
          description: detail.front.description.trim(),
          params,
          asOf: asOfSql(asOf),
        },
      });
      onSavedAs(
        { key: `saved:${id}`, kind: "saved", title: detail.front.id, queryId: `saved:${id}`, id },
        params,
      );
      onChanged();
    } catch (e) {
      onOutput(tab.key, { result: null, error: String(e) });
    } finally {
      setSaving(false);
    }
  }

  async function toggleFavorite() {
    if (!hit) return;
    try {
      await invoke("set_query_meta", { queryId: hit.query_id, favorite: !hit.favorite });
      setHit({ ...hit, favorite: !hit.favorite });
      onChanged();
    } catch (e) {
      onOutput(tab.key, { result: null, error: String(e) });
    }
  }

  if (loadError) return <Banner kind="error">{loadError}</Banner>;
  // Until the query arrives, the editor area is covered and shows that it is opening.
  if (!detail) return <BusyOverlay label={`Opening ${tab.queryId}…`} />;

  const group = groupOf(detail.front.id);

  return (
    <SqlPage
      crumbs={["Catalog", group, detail.front.id]}
      title={detail.front.id}
      actions={
        <>
          <ToolButton
            icon="play"
            label={running ? "Running…" : "Run"}
            primary
            busy={running}
            disabled={running || resolvedDs === null}
            title={resolvedDs !== null ? "Run read-only" : "Choose a data source for this query"}
            onClick={run}
          />
          <ToolButton icon="saveAs" label="Save as" busy={saving} disabled={!detail} title="Save as a saved statement (parameters included)" onClick={saveAs} />
          <ToolButton icon="save" label="Save" disabled title="Catalog queries are read-only. Use Save as." onClick={() => undefined} />
          <ToolSep />
          <ToolButton icon="copy" label="Copy SQL" onClick={() => navigator.clipboard?.writeText(detail.body).catch(() => undefined)} />
          <ToolButton icon="hash" label="Copy id" onClick={() => navigator.clipboard?.writeText(detail.front.id).catch(() => undefined)} />
          <ToolSep />
          {hit ? (
            <ToolButton icon={hit.favorite ? "starFilled" : "star"} label="Favourite" onClick={toggleFavorite} />
          ) : (
            <ToolButton icon="star" label="Favourite" disabled onClick={() => undefined} />
          )}
          <ToolButton icon="trash" label="Delete" disabled title="Catalog queries are files; delete them in the repository." onClick={() => undefined} />
        </>
      }
      fields={[
        { label: "Name", value: detail.front.id },
        { label: "Folder", value: group },
        { label: "Description", value: detail.front.description.trim(), multiline: true },
      ]}
      banners={detail.lint.map((p) => (
        <Banner key={p} kind="warn">
          {p}
        </Banner>
      ))}
      params={
        <form
          className="params-block"
          onSubmit={(e) => {
            e.preventDefault();
            run();
          }}
        >
          <InfoTable>
            <InfoLine label="Data source">
              <DataSourceSelect value={dsChoice} datasources={datasources} onChange={chooseDs} />
            </InfoLine>
            {detail.time_override && (
              <InfoLine label="As of">
                <TimeInput value={asOf} onChange={setAsOf} placeholder="empty = now" />
              </InfoLine>
            )}
            {Object.entries(detail.front.params).map(([name, spec]) => {
              const c = controlFor(spec);
              const enumValues = spec.kind?.enum ?? null;
              return (
                <InfoLine key={name} label={name} required={spec.required}>
                  {enumValues ? (
                    <select value={values[name] ?? ""} onChange={(e) => setValues({ ...values, [name]: e.target.value })}>
                      <option value="">— null —</option>
                      {enumValues.map((v) => (
                        <option key={v}>{v}</option>
                      ))}
                    </select>
                  ) : c.type === "datetime-local" ? (
                    <TimeInput
                      value={values[name] ?? ""}
                      onChange={(v) => setValues({ ...values, [name]: v })}
                      placeholder={spec.description?.trim() ?? "empty = NULL"}
                    />
                  ) : (
                    <input
                      type={c.type}
                      step={c.type === "number" ? "any" : undefined}
                      maxLength={spec.kind?.maxLength ?? undefined}
                      value={values[name] ?? ""}
                      placeholder={spec.description?.trim() ?? "empty = NULL"}
                      onChange={(e) => setValues({ ...values, [name]: e.target.value })}
                    />
                  )}
                </InfoLine>
              );
            })}
          </InfoTable>
          {Object.keys(detail.front.params).length === 0 && !detail.time_override && (
            <span className="meta small">This query has no parameters.</span>
          )}
        </form>
      }
      sql={detail.body}
      readOnly
      onCursor={onCursor}
      sqlEditor={(p) => <SqlEditor {...p} />}
      overlay={running ? <BusyOverlay label="Running…" /> : null}
    />
  );
}
