// Shared types and helpers. Field names follow the Rust structs (snake_case where Rust uses it).

export type Kind = {
  type: string;
  role?: string | null;
  maxLength?: number | null;
  enum?: string[] | null;
};

export type ParamSpec = {
  like?: string | null;
  kind?: Kind | null;
  required: boolean;
  description?: string | null;
};

export type FrontMatter = {
  id: string;
  description: string;
  entities: string[];
  datasets: Record<string, string>;
  params: Record<string, ParamSpec>;
  permissions: string[];
};

/** One row of the catalog search index. */
export type QueryHit = {
  query_id: string;
  path: string;
  description: string;
  tags: string;
  favorite: boolean;
  last_run_at: number | null;
  run_count: number;
};

export type SyncReport = { indexed: number; skipped: string[] };

/** One recorded run. `params_json` holds the exact values used, including "__asOf" when set. */
export type RunRecord = {
  id: number;
  query_id: string;
  params_json: string;
  ran_at: number;
  ok: boolean;
  rows: number | null;
  elapsed_ms: number | null;
  error: string | null;
};

export type QueryDetail = {
  front: FrontMatter;
  body: string;
  lint: string[];
  time_override: boolean;
};

export type QueryResult = {
  columns: string[];
  rows: (string | null)[][];
  truncated: boolean;
  elapsed_ms: number;
  /** The script sent to the database: parameters rendered, inside the read-only transaction. */
  executed_sql: string;
};

/** A PostgreSQL the app can query. The password is in the keychain; `password_saved` says whether one is there. */
export type Datasource = {
  id: number;
  name: string;
  host: string;
  port: number;
  database: string;
  user: string;
  via_ssh: boolean;
  ssh_host: string;
  container: string;
  password_saved: boolean;
};

/** A result kept on disk (its CSV file is in the results folder). */
export type SavedResult = {
  id: number;
  name: string;
  source: string;
  datasource_id: number | null;
  sql: string;
  row_count: number;
  truncated: boolean;
  created_at: number;
  file: string;
  /** The parameter values the result was made with, as JSON. */
  params: string | null;
};

/** The data source a run goes to: the statement's own, else the environment's (nearest), else the default. */
export function resolveDatasource(
  statementDs: number | null | undefined,
  envDs: number | null,
  defaultDs: number | null,
): number | null {
  return statementDs ?? envDs ?? defaultDs ?? null;
}

/** The data source an environment gives, from itself or its nearest parent that sets one. */
export function effectiveDatasource(envs: Environment[], name: string): number | null {
  const byName = new Map(envs.map((e) => [e.name, e]));
  const seen = new Set<string>();
  let cur = byName.get(name);
  while (cur && !seen.has(cur.name)) {
    if (cur.datasource_id != null) return cur.datasource_id;
    seen.add(cur.name);
    cur = cur.parent ? byName.get(cur.parent) : undefined;
  }
  return null;
}

/** A SQL statement the user typed and saved. */
export type SavedSql = {
  id: number;
  /** The data source the statement runs on; null means the environment's or the default one. */
  datasource_id: number | null;
  name: string;
  /** Group the statement is filed under; empty means ungrouped. */
  group: string;
  description: string;
  sql: string;
  created_at: number;
  updated_at: number;
};

export type Settings = {
  catalogDir: string;
  mappingPath: string;
  /** Only read to import the connection used before data sources existed. */
  conn: string;
  activeEnv: string;
  /** The data source used when a statement and its environment choose none. */
  defaultDatasource: number | null;
  /** Folder CSV exports are written to. Empty: ask for a file each time. */
  exportDir: string;
  /** The system's own documents (repo_quizbuks), indexed for the AI analysis. */
  knowledgeDir: string;
  /** The model the AI analysis uses. The API key is in the keychain, not here. */
  aiModel: string;
};

/**
 * A named set of parameter values, optionally under a parent. The parent's values apply
 * unless this environment sets the same parameter. Forms fill from the environment in use.
 */
export type Environment = {
  name: string;
  parent: string | null;
  /** The data source this environment gives (its own, not inherited). */
  datasource_id: number | null;
  description: string;
  /** Only the values this environment sets itself. */
  values: Record<string, string>;
};

/** The values an environment resolves to: its ancestors' values, overridden from the root down. */
export function effectiveValues(envs: Environment[], name: string): Record<string, string> {
  const byName = new Map(envs.map((e) => [e.name, e]));
  const chain: Environment[] = [];
  const seen = new Set<string>();
  let cur = byName.get(name);
  while (cur && !seen.has(cur.name)) {
    seen.add(cur.name);
    chain.unshift(cur);
    cur = cur.parent ? byName.get(cur.parent) : undefined;
  }
  const out: Record<string, string> = {};
  for (const e of chain) Object.assign(out, e.values);
  return out;
}

/** Environments in tree order (a parent, then its children by name), with their depth. */
export function envTree(envs: Environment[]): { env: Environment; depth: number }[] {
  const names = new Set(envs.map((e) => e.name));
  const children = new Map<string, Environment[]>();
  const roots: Environment[] = [];
  for (const e of envs) {
    if (e.parent && names.has(e.parent) && e.parent !== e.name) {
      const list = children.get(e.parent) ?? [];
      list.push(e);
      children.set(e.parent, list);
    } else {
      roots.push(e);
    }
  }
  const byName = (a: Environment, b: Environment) => a.name.localeCompare(b.name, undefined, { sensitivity: "base" });
  const out: { env: Environment; depth: number }[] = [];
  const visited = new Set<string>();
  const walk = (e: Environment, depth: number) => {
    if (visited.has(e.name)) return;
    visited.add(e.name);
    out.push({ env: e, depth });
    for (const c of (children.get(e.name) ?? []).sort(byName)) walk(c, depth + 1);
  };
  for (const r of roots.sort(byName)) walk(r, 0);
  return out;
}

export const DEFAULT_SETTINGS: Settings = {
  catalogDir: "/Users/zhongpingxue/quizbuks/repo_quizbuks/backend/quizbuks/src/main/resources/queries",
  mappingPath: "",
  conn: "",
  activeEnv: "",
  defaultDatasource: null,
  exportDir: "",
  knowledgeDir: "/Users/zhongpingxue/quizbuks/repo_quizbuks",
  aiModel: "claude-sonnet-5-5",
};

const SETTINGS_KEY = "sqlpool.settings";

// Settings live in localStorage for now. The connection string should move to the OS keychain.
export function loadSettings(): Settings {
  try {
    const raw = localStorage.getItem(SETTINGS_KEY);
    return raw ? { ...DEFAULT_SETTINGS, ...JSON.parse(raw) } : DEFAULT_SETTINGS;
  } catch {
    return DEFAULT_SETTINGS;
  }
}

export function saveSettings(s: Settings) {
  try {
    localStorage.setItem(SETTINGS_KEY, JSON.stringify(s));
  } catch {
    /* storage unavailable: settings just won't persist */
  }
}

/** Input type for a declared parameter kind. Unknown kinds fall back to text. */
export function controlFor(spec: ParamSpec): { type: string; hint: string } {
  switch (spec.kind?.type ?? "") {
    case "temporal":
      return { type: "datetime-local", hint: "UTC" };
    case "date":
      return { type: "date", hint: "" };
    case "numeric":
    case "monetary":
      return { type: "number", hint: "" };
    default:
      return { type: "text", hint: "" };
  }
}

const TEMPORAL: ParamSpec = { required: false, kind: { type: "temporal" } };

/** Form value -> SQL text. Temporal form values are UTC (datetime-local has no zone). */
export function toSqlValue(spec: ParamSpec, v: string): string {
  if (spec.kind?.type === "temporal") {
    return v.length === 16 ? `${v}:00Z` : `${v}Z`;
  }
  return v;
}

/** Saved SQL text -> form value. */
export function fromSqlValue(spec: ParamSpec | undefined, v: string | null): string {
  if (v === null) return "";
  if (spec?.kind?.type === "temporal") return v.replace(/Z$/, "").slice(0, 16);
  return v;
}

/** "As of" from the time field of a temporal query. Empty means now. */
export function asOfSql(value: string): string | null {
  return value.trim() ? toSqlValue(TEMPORAL, value.trim()) : null;
}

export function asOfFromRun(saved: Record<string, string | null>): string {
  return fromSqlValue(TEMPORAL, saved["__asOf"] ?? null);
}

export function formatTime(ms: number | null): string {
  if (ms === null) return "never";
  return new Date(ms).toLocaleString();
}

export function parseParams(json: string): Record<string, string | null> {
  try {
    return JSON.parse(json) as Record<string, string | null>;
  } catch {
    return {};
  }
}

/** Names of the `:name` parameters in SQL, skipping strings, comments and `::` casts. */
export function usedParams(sql: string): string[] {
  const names: string[] = [];
  const seen = new Set<string>();
  let i = 0;
  while (i < sql.length) {
    const c = sql[i];
    const next = sql[i + 1];
    if (c === "'" || c === '"') {
      i++;
      while (i < sql.length) {
        if (sql[i] === c) {
          if (sql[i + 1] === c) {
            i += 2;
            continue;
          }
          break;
        }
        i++;
      }
      i++;
    } else if (c === "-" && next === "-") {
      while (i < sql.length && sql[i] !== "\n") i++;
    } else if (c === "/" && next === "*") {
      const end = sql.indexOf("*/", i + 2);
      i = end < 0 ? sql.length : end + 2;
    } else if (c === ":" && next !== ":" && sql[i - 1] !== ":" && /[A-Za-z_]/.test(next ?? "")) {
      let j = i + 1;
      while (j < sql.length && /[A-Za-z0-9_]/.test(sql[j])) j++;
      const name = sql.slice(i + 1, j);
      if (!seen.has(name)) {
        seen.add(name);
        names.push(name);
      }
      i = j;
    } else {
      i++;
    }
  }
  return names;
}

/**
 * An open editor tab. `key` is unique: "catalog:<path>", "saved:<id>", "draft:<n>" (unsaved statement)
 * or "settings". `queryId` is what the run history records the runs under ("" when there is none).
 */
export type Tab = {
  key: string;
  kind: "catalog" | "saved" | "env" | "ds" | "result" | "backup" | "command" | "script" | "analysis" | "generate" | "table" | "settings";
  title: string;
  queryId: string;
  path?: string;
  id?: number | null;
  /** For a new statement: the group it is created in. */
  group?: string;
};

/** What an editor tab last produced: rows, or an error. Shown in the bottom panel. */
export type Output = {
  result: QueryResult | null;
  error: string | null;
  /** Where the result came from, for keeping it: the tab's query id and the data source it ran on. */
  source?: string;
  datasourceId?: number | null;
  /** The parameters the result was made with, as JSON (kept with it). */
  params?: string;
};

/** Values to put back into a tab's form ("Reuse values" or "Open" from a run). */
export type Reuse = { key: string; params: string; nonce: number };

/** A saved-statement folder. `count` is how many statements it holds. */
export type SavedGroup = { name: string; count: number };

/** A request to generate a statement or command, from the editor it came from. */
export type GenerateRequest = {
  kind: "sql" | "command" | "script";
  description: string;
  current: string;
  server: string;
  mappingPath: string;
  /** The editor tab the accepted text goes back to. */
  originKey: string;
};

/** An accepted proposal, waiting for the editor it belongs to. */
export type Accepted = { key: string; text: string; nonce: number };
