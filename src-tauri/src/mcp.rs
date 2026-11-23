//! A read-only MCP server for Claude Desktop: JSON-RPC 2.0 over stdio, one message per line.
//!
//! It offers five tools: search the saved catalog queries, describe one (its parameters and SQL), run one on
//! the configured PostgreSQL database, list the mapped tables and their fields, and run ad-hoc SELECTs over
//! those mapped tables (`adhoc.rs` checks them). It does not write, does not run scripts or server commands.
//! Every run is read-only (see `db::readonly_script`) and recorded.
//!
//! It keeps its own index and run history in `sqlpool-mcp.db`, next to its config file, so it never writes
//! to the app's database. The database password is in the keychain (`--set-password`), not in the config.

use crate::catalog;
use crate::connection;
use crate::db;
use crate::secrets;
use crate::store::{self, RunInput};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// Keychain account for the database password of the MCP server.
pub const PASSWORD_ACCOUNT: &str = "mcp:postgres";
const PROTOCOL_VERSION: &str = "2024-11-05";
const DEFAULT_ROWS: usize = 100;
const MAX_ROWS: usize = 500;
const SEARCH_LIMIT: usize = 50;
const STATEMENT_TIMEOUT_MS: u32 = 30_000;

/// `mcp.json` in the app's data folder.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileConfig {
    /// The catalog folder with the `.sql` query files.
    pub catalog_dir: String,
    /// The JSON mapping from `{{Entity}}` to physical SQL (see tools/gen_mapping.py). Optional.
    pub mapping_path: Option<String>,
    pub database: DatabaseConfig,
}

#[derive(Deserialize)]
pub struct DatabaseConfig {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: i64,
    pub database: String,
    pub user: String,
}

fn default_port() -> i64 {
    5432
}

pub struct Server {
    pub conn: Connection,
    pub mapping_path: Option<String>,
    pub database_url: String,
    pub runtime: tokio::runtime::Runtime,
}

/// Entry point for the `sqlpool-mcp` binary. Returns the exit code.
pub fn main_with_args(args: &[String]) -> i32 {
    if args.first().map(String::as_str) == Some("--set-password") {
        return match set_password() {
            Ok(()) => {
                eprintln!("sqlpool-mcp: the password is saved in the keychain");
                0
            }
            Err(e) => {
                eprintln!("sqlpool-mcp: {e}");
                1
            }
        };
    }
    let config_path = option_value(args, "--config")
        .map(PathBuf::from)
        .unwrap_or_else(default_config_path);
    match start(&config_path) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("sqlpool-mcp: {e}");
            1
        }
    }
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.get(i + 1).cloned()
}

/// `~/Library/Application Support/com.sqlpool.app/mcp.json`, the folder the app uses.
pub fn default_config_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join("Library/Application Support/com.sqlpool.app/mcp.json")
}

/// Reads one line from stdin and saves it as the database password. Nothing is printed back.
fn set_password() -> Result<(), String> {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .map_err(|e| format!("read the password: {e}"))?;
    let password = line.trim_end_matches(['\r', '\n']);
    if password.is_empty() {
        return Err("no password given on stdin".into());
    }
    secrets::set_named(PASSWORD_ACCOUNT, password)
}

fn start(config_path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(config_path).map_err(|e| format!("config {}: {e}", config_path.display()))?;
    let file: FileConfig = serde_json::from_str(&text).map_err(|e| format!("config {}: {e}", config_path.display()))?;

    let dir = config_path.parent().unwrap_or(Path::new("."));
    let mut conn = store::open(&dir.join("sqlpool-mcp.db"))?;
    let report = store::sync_catalog(&mut conn, Path::new(&file.catalog_dir))?;
    for skipped in &report.skipped {
        eprintln!("sqlpool-mcp: skipped {skipped}");
    }
    eprintln!("sqlpool-mcp: {} queries indexed", report.indexed);

    let password = secrets::get_named(PASSWORD_ACCOUNT)?.unwrap_or_default();
    let db = &file.database;
    let database_url = connection::postgres_url(&db.host, db.port, &db.database, &db.user, &password);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))?;
    let mut server = Server { conn, mapping_path: file.mapping_path, database_url, runtime };
    serve(&mut server)
}

/// Reads requests from stdin and writes replies to stdout until stdin closes.
pub fn serve(server: &mut Server) -> Result<(), String> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("stdin: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(server, &line) {
            writeln!(out, "{reply}").and_then(|_| out.flush()).map_err(|e| format!("stdout: {e}"))?;
        }
    }
    Ok(())
}

/// One JSON-RPC message in, its reply out. Notifications (no `id`) get no reply.
pub fn handle_line(server: &mut Server, line: &str) -> Option<String> {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return Some(error_reply(Value::Null, -32700, &format!("parse error: {e}"))),
    };
    let id = msg.get("id").cloned()?;
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return Some(error_reply(id, -32600, "invalid request: no method"));
    };
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "sqlpool", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Read-only access to the database. Prefer saved queries: search_queries, describe_query, run_query. If none fits, use list_tables to find the mapped tables and fields, then run_sql with one SELECT over them.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_list() })),
        "tools/call" => Ok(call_tool(server, msg.get("params").unwrap_or(&Value::Null))),
        other => Err((-32601, format!("method not found: {other}"))),
    };
    Some(match result {
        Ok(value) => ok_reply(id, value),
        Err((code, message)) => error_reply(id, code, &message),
    })
}

fn ok_reply(id: Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error_reply(id: Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

fn tool_list() -> Value {
    json!([
        {
            "name": "search_queries",
            "description": "Search the saved queries by words in their description, tags or SQL. Several words must all match. Returns ids for describe_query and run_query.",
            "inputSchema": {
                "type": "object",
                "properties": { "text": { "type": "string", "description": "Words to search for. Empty lists the queries." } }
            }
        },
        {
            "name": "describe_query",
            "description": "Show one saved query: its description, its parameters (name, type, required, description), its SQL and any lint warnings.",
            "inputSchema": {
                "type": "object",
                "properties": { "id": { "type": "string", "description": "The query id from search_queries." } },
                "required": ["id"]
            }
        },
        {
            "name": "run_query",
            "description": "Run one saved query read-only on the configured PostgreSQL database and return its rows. Give parameters by name; leave one out or pass null for an empty value. Rows are capped (default 100, at most 500).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": { "type": "string", "description": "The query id from search_queries." },
                    "params": {
                        "type": "object",
                        "description": "Parameter values by name. Numbers and booleans are sent as text.",
                        "additionalProperties": { "type": ["string", "number", "boolean", "null"] }
                    },
                    "asOf": { "type": "string", "description": "ISO 8601 time for a query that reads temporal data. Empty means now." },
                    "maxRows": { "type": "integer", "minimum": 1, "maximum": MAX_ROWS }
                },
                "required": ["id"]
            }
        },
        {
            "name": "list_tables",
            "description": "Without a name: list the mapped tables (entities), optionally filtered by a search word. With a name: that table's physical table and its fields with their columns. Use this before run_sql.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "An entity name from the list, to see its fields." },
                    "search": { "type": "string", "description": "Words to match against entity names and tables." }
                }
            }
        },
        {
            "name": "run_sql",
            "description": "Run one ad-hoc SELECT (or WITH ... SELECT) read-only. Only the mapped tables may be read; use list_tables to see them. Write the table as its entity name in double braces, e.g. {{Order}}, or by its physical name. Values go in as literals; :name parameters are not supported here. Rows are capped (default 100, at most 500). The result shows the SQL that ran.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": { "type": "string", "description": "One SELECT statement." },
                    "asOf": { "type": "string", "description": "ISO 8601 time for temporal entities. Empty means now." },
                    "maxRows": { "type": "integer", "minimum": 1, "maximum": MAX_ROWS }
                },
                "required": ["sql"]
            }
        }
    ])
}

/// A tool call: the text of the result, or an error shown to the model (`isError`).
fn call_tool(server: &mut Server, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let outcome = match name {
        "search_queries" => search_queries(server, &args),
        "describe_query" => describe_query(server, &args),
        "run_query" => run_query(server, &args),
        "list_tables" => list_tables(server, &args),
        "run_sql" => run_sql(server, &args),
        other => Err(format!("unknown tool: {other}")),
    };
    match outcome {
        Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("argument {key} (string) is required"))
}

fn to_text(value: &Value) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

fn search_queries(server: &Server, args: &Value) -> Result<String, String> {
    let text = args.get("text").and_then(Value::as_str).unwrap_or("");
    let hits = store::search(&server.conn, text)?;
    let total = hits.len();
    let queries: Vec<Value> = hits
        .iter()
        .take(SEARCH_LIMIT)
        .map(|h| {
            json!({
                "id": h.query_id,
                "description": h.description,
                "tags": h.tags,
                "favorite": h.favorite,
                "runCount": h.run_count,
            })
        })
        .collect();
    to_text(&json!({ "total": total, "shown": queries.len(), "queries": queries }))
}

fn describe_query(server: &Server, args: &Value) -> Result<String, String> {
    let id = str_arg(args, "id")?;
    let hit = store::find(&server.conn, id)?.ok_or_else(|| format!("no query with id {id}"))?;
    let (front, body) = read_query(&hit.path)?;
    let mapping = load_mapping(server.mapping_path.as_deref())?;
    let params: Vec<Value> = front
        .params
        .iter()
        .map(|(name, spec)| {
            let kind = spec.kind.clone().unwrap_or_default();
            json!({
                "name": name,
                "type": kind.ty,
                "required": spec.required,
                "description": spec.description,
                "maxLength": kind.max_length,
                "enum": kind.enum_values,
            })
        })
        .collect();
    to_text(&json!({
        "id": front.id,
        "description": front.description,
        "params": params,
        "timeSlice": front.time_slice,
        "temporal": catalog::uses_temporal(&body, &mapping),
        "lint": catalog::lint(&front, &body, &mapping),
        "sql": body,
    }))
}

fn run_query(server: &mut Server, args: &Value) -> Result<String, String> {
    let id = str_arg(args, "id")?;
    let hit = store::find(&server.conn, id)?.ok_or_else(|| format!("no query with id {id}"))?;
    let (front, body) = read_query(&hit.path)?;
    let mapping = load_mapping(server.mapping_path.as_deref())?;

    let given = match args.get("params") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err("argument params must be an object".into()),
    };
    let mut values: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (name, v) in given {
        if !front.params.contains_key(&name) {
            return Err(format!("query {id} has no parameter {name}"));
        }
        let text = match v {
            Value::Null => None,
            Value::String(s) => Some(s),
            Value::Bool(b) => Some(b.to_string()),
            Value::Number(n) => Some(n.to_string()),
            other => return Err(format!("parameter {name}: expected text, a number, a boolean or null, got {other}")),
        };
        values.insert(name, text.filter(|s| !s.is_empty()));
    }
    for (name, spec) in &front.params {
        if spec.required && values.get(name).is_none_or(|v| v.is_none()) {
            return Err(format!("parameter {name} is required"));
        }
    }

    let as_of = args
        .get("asOf")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let max_rows = args
        .get("maxRows")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1, MAX_ROWS))
        .unwrap_or(DEFAULT_ROWS);

    let time = catalog::run_time(&front.time_slice, &values, as_of.as_deref())?;
    let resolved = catalog::resolve_placeholders(&body, &mapping, &time).map_err(|names| {
        format!(
            "placeholders not in mapping: {}",
            names.iter().map(|n| format!("{{{{{n}}}}}")).collect::<Vec<_>>().join(", ")
        )
    })?;
    let sql = catalog::render_sql(&resolved, &values)?;
    let executed = db::readonly_script(&sql, STATEMENT_TIMEOUT_MS);
    let outcome = server
        .runtime
        .block_on(db::run_readonly(&server.database_url, &sql, STATEMENT_TIMEOUT_MS, max_rows));

    // Every run that got this far is recorded, successful or not.
    let params_json = serde_json::to_string(&values).unwrap_or_else(|_| "{}".into());
    let (ok, rows, elapsed_ms, error) = match &outcome {
        Ok(r) => (true, Some(r.rows.len() as i64), Some(r.elapsed_ms as i64), None),
        Err(e) => (false, None, None, Some(e.as_str())),
    };
    let _ = store::record_run(
        &server.conn,
        &RunInput {
            sql: &executed,
            query_id: &front.id,
            path: hit.path.as_str(),
            description: &front.description,
            params_json: &params_json,
            ok,
            rows,
            elapsed_ms,
            error,
        },
    );

    to_text(&serde_json::to_value(outcome?).map_err(|e| e.to_string())?)
}

const TABLE_LIMIT: usize = 200;

/// The mapped entities (plain and temporal) and their fields. `name` shows one entity's fields.
fn list_tables(server: &Server, args: &Value) -> Result<String, String> {
    let mapping = load_mapping(server.mapping_path.as_deref())?;
    let fields_of = |entity: &str| -> Vec<Value> {
        let prefix = format!("{entity}.");
        mapping
            .tables
            .iter()
            .filter_map(|(k, column)| k.strip_prefix(&prefix).map(|f| json!({ "name": f, "column": column })))
            .collect()
    };

    if let Some(name) = args.get("name").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
        if let Some(t) = mapping.temporal.get(name) {
            return to_text(&json!({
                "entity": name,
                "table": t.table,
                "temporal": true,
                "primaryKey": t.pk,
                "systemColumns": t.columns,
                "fields": fields_of(name),
            }));
        }
        let table = mapping
            .tables
            .get(name)
            .ok_or_else(|| format!("no entity named {name} in the mapping"))?;
        return to_text(&json!({ "entity": name, "table": table, "temporal": false, "fields": fields_of(name) }));
    }

    let search = args.get("search").and_then(Value::as_str).unwrap_or("").trim().to_lowercase();
    let mut entities: Vec<(String, String, bool)> = mapping
        .tables
        .iter()
        .filter(|(k, _)| !k.contains('.'))
        .map(|(k, v)| (k.clone(), v.clone(), false))
        .chain(mapping.temporal.iter().map(|(k, t)| (k.clone(), t.table.clone(), true)))
        .filter(|(name, table, _)| {
            search.is_empty() || name.to_lowercase().contains(&search) || table.to_lowercase().contains(&search)
        })
        .collect();
    entities.sort();
    let total = entities.len();
    let shown: Vec<Value> = entities
        .iter()
        .take(TABLE_LIMIT)
        .map(|(name, table, temporal)| {
            json!({ "name": name, "table": table, "temporal": temporal, "fieldCount": fields_of(name).len() })
        })
        .collect();
    to_text(&json!({ "total": total, "shown": shown.len(), "entities": shown }))
}

/// The physical names a run_sql statement may read: the mapped tables and temporal entities' tables.
fn allowed_tables(mapping: &catalog::Mapping) -> std::collections::HashSet<String> {
    let plain = mapping
        .tables
        .iter()
        .filter(|(k, _)| !k.contains('.'))
        .map(|(_, v)| v.to_lowercase());
    let temporal = mapping.temporal.values().map(|t| t.table.to_lowercase());
    plain.chain(temporal).collect()
}

fn run_sql(server: &mut Server, args: &Value) -> Result<String, String> {
    let sql = str_arg(args, "sql")?;
    let mapping = load_mapping(server.mapping_path.as_deref())?;
    let as_of = args
        .get("asOf")
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let max_rows = args
        .get("maxRows")
        .and_then(Value::as_u64)
        .map(|n| (n as usize).clamp(1, MAX_ROWS))
        .unwrap_or(DEFAULT_ROWS);

    let no_time_slice: Option<catalog::TimeSliceSpec> = None;
    let time = catalog::run_time(&no_time_slice, &BTreeMap::new(), as_of.as_deref())?;
    let resolved = catalog::resolve_placeholders(sql, &mapping, &time).map_err(|names| {
        format!(
            "placeholders not in mapping: {}",
            names.iter().map(|n| format!("{{{{{n}}}}}")).collect::<Vec<_>>().join(", ")
        )
    })?;
    if !catalog::used_params(&resolved).is_empty() {
        return Err("run_sql does not take :name parameters; write the values into the SQL as literals".into());
    }
    if allowed_tables(&mapping).is_empty() {
        return Err("no tables are mapped: set mappingPath in mcp.json".into());
    }
    crate::adhoc::check(&resolved, &allowed_tables(&mapping))?;

    let executed = db::readonly_script(&resolved, STATEMENT_TIMEOUT_MS);
    let outcome = server
        .runtime
        .block_on(db::run_readonly(&server.database_url, &resolved, STATEMENT_TIMEOUT_MS, max_rows));

    let (ok, rows, elapsed_ms, error) = match &outcome {
        Ok(r) => (true, Some(r.rows.len() as i64), Some(r.elapsed_ms as i64), None),
        Err(e) => (false, None, None, Some(e.as_str())),
    };
    let _ = store::record_run(
        &server.conn,
        &RunInput {
            sql: &executed,
            query_id: "adhoc",
            path: "",
            description: "Ad-hoc SQL (run_sql)",
            params_json: "{}",
            ok,
            rows,
            elapsed_ms,
            error,
        },
    );

    to_text(&serde_json::to_value(outcome?).map_err(|e| e.to_string())?)
}

fn read_query(path: &str) -> Result<(catalog::FrontMatter, String), String> {
    let source = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    catalog::parse_source(&source)
}

/// The mapping file, read on each call so a change to it applies without a restart.
fn load_mapping(path: Option<&str>) -> Result<catalog::Mapping, String> {
    let Some(p) = path.filter(|p| !p.trim().is_empty()) else {
        return Ok(catalog::Mapping::default());
    };
    let text = fs::read_to_string(p).map_err(|e| format!("mapping file {p}: {e}"))?;
    catalog::parse_mapping(&text).map_err(|e| format!("mapping file {p}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    const COUNT_QUERY: &str = "/*---\nid: t.count\ndescription: Count items with a status\nparams:\n  status: { kind: { type: string }, required: true, description: The status }\n---*/\nSELECT count(*) AS n FROM items WHERE status = :status\n";

    /// A server over an in-memory index of a temporary catalog. The database is unreachable on purpose.
    fn server_with_catalog() -> Server {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("sqlpool-mcp-test-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("count.sql"), COUNT_QUERY).unwrap();
        let mut conn = Connection::open_in_memory().unwrap();
        store::migrate(&conn).unwrap();
        store::sync_catalog(&mut conn, &dir).unwrap();
        Server {
            conn,
            mapping_path: None,
            database_url: "postgres://u:p@127.0.0.1:1/none".into(),
            runtime: tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(),
        }
    }

    fn call(server: &mut Server, method: &str, params: Value) -> Value {
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
        serde_json::from_str(&handle_line(server, &line).expect("a reply")).unwrap()
    }

    fn tool_text(reply: &Value) -> (String, bool) {
        let result = &reply["result"];
        (
            result["content"][0]["text"].as_str().unwrap().to_string(),
            result["isError"].as_bool().unwrap(),
        )
    }

    #[test]
    fn initializes_and_lists_five_tools() {
        let mut s = server_with_catalog();
        let init = call(&mut s, "initialize", json!({}));
        assert_eq!(init["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(init["result"]["capabilities"]["tools"].is_object());

        let list = call(&mut s, "tools/list", json!({}));
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["search_queries", "describe_query", "run_query", "list_tables", "run_sql"]);
    }

    /// A server with a mapping file: Order -> orders (fields id, total), and the temporal Price.
    fn server_with_mapping() -> Server {
        let mut s = server_with_catalog();
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("sqlpool-mcp-map-{nanos}.json"));
        fs::write(
            &path,
            r#"{"Order": "orders", "Order.id": "id", "Order.total": "total",
                "Customer": "customers",
                "Price": {"table": "price_version", "pk": "price_id", "writeOnce": false, "columns": {"versionNo": "version_no"}}}"#,
        )
        .unwrap();
        s.mapping_path = Some(path.to_string_lossy().to_string());
        s
    }

    #[test]
    fn list_tables_lists_entities_and_fields() {
        let mut s = server_with_mapping();
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "list_tables", "arguments": {}})));
        assert!(!err);
        assert!(text.contains("\"name\":\"Order\"") && text.contains("\"name\":\"Price\""), "{text}");

        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "list_tables", "arguments": {"name": "Order"}})));
        assert!(!err);
        assert!(text.contains("\"column\":\"total\"") && text.contains("\"table\":\"orders\""), "{text}");

        let (_, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "list_tables", "arguments": {"name": "Nope"}})));
        assert!(err);
    }

    #[test]
    fn run_sql_only_reads_mapped_tables() {
        let mut s = server_with_mapping();
        // A mapped entity placeholder and the physical name both pass the check; the DB is unreachable,
        // so the error is the connection, which shows the statement got past the guard.
        let ok = json!({"sql": "SELECT count(*) FROM {{Order}} o JOIN customers c ON c.id = o.id"});
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "run_sql", "arguments": ok})));
        assert!(err);
        assert!(text.contains("connect"), "{text}");

        let bad = json!({"sql": "SELECT * FROM secret_table"});
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "run_sql", "arguments": bad})));
        assert!(err);
        assert!(text.contains("not one of the mapped tables"), "{text}");

        let write = json!({"sql": "DELETE FROM orders"});
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "run_sql", "arguments": write})));
        assert!(err);
        assert!(text.contains("SELECT"), "{text}");

        let param = json!({"sql": "SELECT * FROM orders WHERE id = :id"});
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "run_sql", "arguments": param})));
        assert!(err);
        assert!(text.contains("parameters"), "{text}");
    }

    #[test]
    fn run_sql_refuses_when_no_mapping_is_set() {
        let mut s = server_with_catalog();
        let args = json!({"sql": "SELECT 1 FROM orders"});
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "run_sql", "arguments": args})));
        assert!(err);
        assert!(text.contains("no tables are mapped"), "{text}");
    }

    #[test]
    fn notifications_get_no_reply_and_unknown_methods_error() {
        let mut s = server_with_catalog();
        assert!(handle_line(&mut s, r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let reply = call(&mut s, "nope/nope", json!({}));
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[test]
    fn searches_and_describes() {
        let mut s = server_with_catalog();
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "search_queries", "arguments": {"text": "items"}})));
        assert!(!err);
        assert!(text.contains("\"id\":\"t.count\""), "{text}");

        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "describe_query", "arguments": {"id": "t.count"}})));
        assert!(!err);
        assert!(text.contains("\"name\":\"status\"") && text.contains("\"required\":true"), "{text}");
    }

    #[test]
    fn run_checks_parameters_before_touching_the_database() {
        let mut s = server_with_catalog();
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "run_query", "arguments": {"id": "t.count"}})));
        assert!(err);
        assert!(text.contains("status is required"), "{text}");

        let args = json!({"id": "t.count", "params": {"status": "x", "nope": "y"}});
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "run_query", "arguments": args})));
        assert!(err);
        assert!(text.contains("has no parameter nope"), "{text}");
    }

    #[test]
    fn run_reports_connection_errors_and_records_the_run() {
        let mut s = server_with_catalog();
        let args = json!({"id": "t.count", "params": {"status": "open"}});
        let (text, err) = tool_text(&call(&mut s, "tools/call", json!({"name": "run_query", "arguments": args})));
        assert!(err);
        assert!(text.contains("connect"), "{text}");
        let runs: i64 = s.conn.query_row("SELECT COUNT(*) FROM run_history", [], |r| r.get(0)).unwrap();
        assert_eq!(runs, 1);
    }

    #[test]
    fn unknown_tool_is_an_error_result_not_a_protocol_error() {
        let mut s = server_with_catalog();
        let reply = call(&mut s, "tools/call", json!({"name": "write_table", "arguments": {}}));
        assert!(reply.get("error").is_none());
        assert!(tool_text(&reply).1);
    }
}
