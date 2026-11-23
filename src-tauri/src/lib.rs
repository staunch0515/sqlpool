mod adhoc;
mod ai;
mod backup;
mod catalog;
mod connection;
mod csv;
mod db;
mod generate;
mod knowledge;
mod local_script;
mod mcp;
mod remote;
mod secrets;
mod server_commands;
mod versioned;
mod store;

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{Emitter, Manager, State};

use store::{QueryHit, RunInput, RunRecord, Store, SyncReport};

#[derive(serde::Serialize)]
struct QueryDetail {
    front: catalog::FrontMatter,
    body: String,
    lint: Vec<String>,
    /// The query reads temporal entities and has no timeSlice: the UI shows an "As of" field.
    time_override: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunArgs {
    path: String,
    /// JSON object: `{"Entity.field": "physical_sql", "Entity": "schema.table"}`.
    mapping_path: Option<String>,
    /// The data source to run on.
    datasource_id: i64,
    /// Empty strings are treated as NULL.
    params: BTreeMap<String, Option<String>>,
    /// "As of" for queries without a timeSlice; empty or absent means now.
    as_of: Option<String>,
    #[serde(default = "default_timeout")]
    statement_timeout_ms: u32,
    #[serde(default = "default_max_rows")]
    max_rows: usize,
}

fn default_timeout() -> u32 {
    30_000
}

fn default_max_rows() -> usize {
    1_000
}

/// The connection string for a data source: its password comes from the keychain.
fn connection_for(store: &State<'_, Store>, datasource_id: i64) -> Result<String, String> {
    let ds = {
        let conn = lock(store)?;
        store::get_datasource(&conn, datasource_id)?
    }
    .ok_or("the data source no longer exists")?;
    let password = if ds.via_ssh {
        String::new()
    } else {
        secrets::get(ds.id)?.unwrap_or_default()
    };
    Ok(connection::for_datasource(&ds, &password))
}

/// The last mapping file read, with its modification time. The file is large (thousands of entries),
/// and every open and run needs it, so it is parsed once and reused until the file changes.
static MAPPING_CACHE: Mutex<Option<(String, std::time::SystemTime, Arc<catalog::Mapping>)>> = Mutex::new(None);

fn load_mapping(path: Option<&str>) -> Result<Arc<catalog::Mapping>, String> {
    let Some(p) = path.filter(|p| !p.trim().is_empty()) else {
        return Ok(Arc::new(catalog::Mapping::default()));
    };
    let modified = fs::metadata(p)
        .and_then(|m| m.modified())
        .map_err(|e| format!("mapping file {p}: {e}"))?;
    let mut cache = MAPPING_CACHE.lock().map_err(|_| "mapping cache poisoned".to_string())?;
    if let Some((_, _, mapping)) = cache.as_ref().filter(|(cp, ct, _)| cp == p && *ct == modified) {
        return Ok(mapping.clone());
    }
    let text = fs::read_to_string(p).map_err(|e| format!("mapping file {p}: {e}"))?;
    let mapping = Arc::new(catalog::parse_mapping(&text).map_err(|e| format!("mapping file {p}: {e}"))?);
    *cache = Some((p.to_string(), modified, mapping.clone()));
    Ok(mapping)
}

fn lock<'a>(store: &'a State<'_, Store>) -> Result<std::sync::MutexGuard<'a, rusqlite::Connection>, String> {
    store.0.lock().map_err(|_| "store lock poisoned".to_string())
}

/// Re-reads the catalog folder and rebuilds the search index.
#[tauri::command]
fn sync_catalog(dir: String, store: State<'_, Store>) -> Result<SyncReport, String> {
    let mut conn = lock(&store)?;
    store::sync_catalog(&mut conn, Path::new(&dir))
}

#[tauri::command]
fn search_queries(text: String, store: State<'_, Store>) -> Result<Vec<QueryHit>, String> {
    let conn = lock(&store)?;
    store::search(&conn, &text)
}

#[tauri::command]
fn set_query_meta(
    query_id: String,
    favorite: Option<bool>,
    tags: Option<String>,
    store: State<'_, Store>,
) -> Result<(), String> {
    let conn = lock(&store)?;
    store::set_meta(&conn, &query_id, favorite, tags.as_deref())
}

#[tauri::command]
fn query_history(query_id: String, limit: Option<i64>, store: State<'_, Store>) -> Result<Vec<RunRecord>, String> {
    let conn = lock(&store)?;
    store::history(&conn, &query_id, limit.unwrap_or(20))
}

#[tauri::command]
fn get_query(path: String, mapping_path: Option<String>) -> Result<QueryDetail, String> {
    let source = fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let (front, body) = catalog::parse_source(&source)?;
    let mapping = load_mapping(mapping_path.as_deref())?;
    let lint = catalog::lint(&front, &body, &mapping);
    let time_override = front.time_slice.is_none() && catalog::uses_temporal(&body, &mapping);
    Ok(QueryDetail { front, body, lint, time_override })
}

#[tauri::command]
async fn run_query(args: RunArgs, store: State<'_, Store>) -> Result<db::QueryResult, String> {
    let source = fs::read_to_string(&args.path).map_err(|e| format!("{}: {e}", args.path))?;
    let (front, body) = catalog::parse_source(&source)?;
    let mapping = load_mapping(args.mapping_path.as_deref())?;

    // Normalize: empty text means "not given".
    let values: BTreeMap<String, Option<String>> = args
        .params
        .into_iter()
        .map(|(k, v)| (k, v.filter(|s| !s.is_empty())))
        .collect();

    for (name, spec) in &front.params {
        if spec.required && values.get(name).is_none_or(|v| v.is_none()) {
            return Err(format!("parameter {name} is required"));
        }
    }

    let time = catalog::run_time(&front.time_slice, &values, args.as_of.as_deref())?;
    let resolved = catalog::resolve_placeholders(&body, &mapping, &time).map_err(|names| {
        format!(
            "placeholders not in mapping: {}",
            names
                .iter()
                .map(|n| format!("{{{{{n}}}}}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let sql = catalog::render_sql(&resolved, &values)?;
    let executed = db::readonly_script(&sql, args.statement_timeout_ms);
    let conn_str = connection_for(&store, args.datasource_id)?;

    let outcome = db::run_readonly(&conn_str, &sql, args.statement_timeout_ms, args.max_rows).await;

    // Record every run that got this far, successful or not. Recording failures must not hide the result.
    // Record the time used too, so "Reuse values" can restore it.
    let mut recorded = values.clone();
    if let (None, Some(v)) = (&front.time_slice, args.as_of.as_deref().filter(|v| !v.trim().is_empty())) {
        recorded.insert("__asOf".to_string(), Some(v.to_string()));
    }
    let params_json = serde_json::to_string(&recorded).unwrap_or_else(|_| "{}".to_string());
    if let Ok(conn) = lock(&store) {
        let (ok, rows, elapsed, error) = match &outcome {
            Ok(r) => (true, Some(r.rows.len() as i64), Some(r.elapsed_ms as i64), None),
            Err(e) => (false, None, None, Some(e.as_str())),
        };
        let _ = store::record_run(
            &conn,
            &RunInput {
                sql: &executed,
                query_id: &front.id,
                path: &args.path,
                description: &front.description,
                params_json: &params_json,
                ok,
                rows,
                elapsed_ms: elapsed,
                error,
            },
        );
    }

    outcome
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveAsArgs {
    path: String,
    mapping_path: Option<String>,
    name: String,
    group: String,
    description: String,
    /// The parameter values as the run would send them (SQL text, null for empty).
    params: BTreeMap<String, Option<String>>,
    as_of: Option<String>,
}

/// "Save as" for a catalog query: stores it as a saved statement. Placeholders are resolved through
/// the mapping and the time of the read is fixed to the As of value; `:name` parameters stay parameters.
#[tauri::command]
fn save_catalog_as(args: SaveAsArgs, store: State<'_, Store>) -> Result<i64, String> {
    let source = fs::read_to_string(&args.path).map_err(|e| format!("{}: {e}", args.path))?;
    let (front, body) = catalog::parse_source(&source)?;
    let mapping = load_mapping(args.mapping_path.as_deref())?;
    let values: BTreeMap<String, Option<String>> = args
        .params
        .into_iter()
        .map(|(k, v)| (k, v.filter(|s| !s.is_empty())))
        .collect();
    let time = catalog::run_time(&front.time_slice, &values, args.as_of.as_deref())?;
    let resolved = catalog::resolve_placeholders(&body, &mapping, &time).map_err(|names| {
        format!(
            "placeholders not in mapping: {}",
            names.iter().map(|n| format!("{{{{{n}}}}}")).collect::<Vec<_>>().join(", ")
        )
    })?;
    let conn = lock(&store)?;
    store::save_sql(&conn, None, &args.name, &args.group, &args.description, resolved.trim())
}

/// A catalog query by id, with its file path and index data. None if it is not in the index.
#[tauri::command]
fn find_query(query_id: String, store: State<'_, Store>) -> Result<Option<QueryHit>, String> {
    let conn = lock(&store)?;
    store::find(&conn, &query_id)
}

#[tauri::command]
fn list_saved_sql(store: State<'_, Store>) -> Result<Vec<store::SavedSql>, String> {
    let conn = lock(&store)?;
    store::list_saved(&conn)
}

#[tauri::command]
fn save_saved_sql(
    id: Option<i64>,
    name: String,
    group: String,
    description: String,
    sql: String,
    store: State<'_, Store>,
) -> Result<i64, String> {
    let conn = lock(&store)?;
    store::save_sql(&conn, id, &name, &group, &description, &sql)
}

#[tauri::command]
fn list_saved_groups(store: State<'_, Store>) -> Result<Vec<store::SavedGroup>, String> {
    let conn = lock(&store)?;
    store::list_groups(&conn)
}

#[tauri::command]
fn create_saved_group(name: String, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::create_group(&conn, &name)
}

#[tauri::command]
fn delete_saved_group(name: String, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::delete_group(&conn, &name)
}

#[tauri::command]
fn list_environments(store: State<'_, Store>) -> Result<Vec<store::Environment>, String> {
    let conn = lock(&store)?;
    store::list_environments(&conn)
}

#[tauri::command]
fn save_environment(
    name: String,
    parent: Option<String>,
    description: String,
    values: BTreeMap<String, String>,
    datasource_id: Option<i64>,
    store: State<'_, Store>,
) -> Result<(), String> {
    let conn = lock(&store)?;
    store::save_environment(&conn, &name, parent.as_deref(), &description, &values, datasource_id)
}

#[tauri::command]
fn rename_environment(old: String, new: String, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::rename_environment(&conn, &old, &new)
}

#[tauri::command]
fn delete_environment(name: String, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::delete_environment(&conn, &name)
}

#[tauri::command]
fn delete_saved_sql(id: i64, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::delete_sql(&conn, id)
}

#[tauri::command]
fn recent_runs(limit: Option<i64>, store: State<'_, Store>) -> Result<Vec<RunRecord>, String> {
    let conn = lock(&store)?;
    store::recent_runs(&conn, limit.unwrap_or(100))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SavedRunArgs {
    id: i64,
    datasource_id: i64,
    /// Values for the `:name` parameters of the statement. Missing or empty means NULL.
    params: BTreeMap<String, Option<String>>,
    #[serde(default = "default_timeout")]
    statement_timeout_ms: u32,
    #[serde(default = "default_max_rows")]
    max_rows: usize,
}

/// Runs a saved statement read-only and records the run with the exact parameter values used.
#[tauri::command]
async fn run_saved_sql(args: SavedRunArgs, store: State<'_, Store>) -> Result<db::QueryResult, String> {
    let saved = {
        let conn = lock(&store)?;
        store::list_saved(&conn)?
            .into_iter()
            .find(|s| s.id == args.id)
            .ok_or_else(|| format!("saved SQL {} not found", args.id))?
    };
    // Every `:name` in the statement gets a value; missing ones are NULL.
    let mut values: BTreeMap<String, Option<String>> = catalog::used_params(&saved.sql)
        .into_iter()
        .map(|name| (name, None))
        .collect();
    for (name, v) in args.params {
        values.insert(name, v.filter(|s| !s.is_empty()));
    }
    let sql = catalog::render_sql(&saved.sql, &values)?;
    let executed = db::readonly_script(&sql, args.statement_timeout_ms);
    let conn_str = connection_for(&store, args.datasource_id)?;
    let outcome = db::run_readonly(&conn_str, &sql, args.statement_timeout_ms, args.max_rows).await;

    let params_json = serde_json::to_string(&values).unwrap_or_else(|_| "{}".to_string());
    let query_id = format!("saved:{}", saved.id);
    if let Ok(conn) = lock(&store) {
        let (ok, rows, elapsed, error) = match &outcome {
            Ok(r) => (true, Some(r.rows.len() as i64), Some(r.elapsed_ms as i64), None),
            Err(e) => (false, None, None, Some(e.as_str())),
        };
        let _ = store::record_run(
            &conn,
            &RunInput {
                sql: &executed,
                query_id: &query_id,
                path: "",
                description: &saved.name,
                params_json: &params_json,
                ok,
                rows,
                elapsed_ms: elapsed,
                error,
            },
        );
    }
    outcome
}

/// A data source as the editor sends it. The password is separate: it goes to the keychain, not the database.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DatasourceArgs {
    id: Option<i64>,
    #[serde(flatten)]
    input: store::DatasourceInput,
    /// `None`: keep the saved password. `Some("")`: remove it. Otherwise: store this one.
    password: Option<String>,
}

#[tauri::command]
fn list_datasources(store: State<'_, Store>) -> Result<Vec<store::Datasource>, String> {
    let conn = lock(&store)?;
    store::list_datasources(&conn)
}

#[tauri::command]
fn save_datasource(args: DatasourceArgs, store: State<'_, Store>) -> Result<i64, String> {
    let conn = lock(&store)?;
    let had_password = match args.id {
        Some(id) => store::get_datasource(&conn, id)?.is_some_and(|d| d.password_saved),
        None => false,
    };
    // Write the password first: if the keychain refuses, nothing is saved.
    let password_saved = match args.password.as_deref() {
        Some("") => false,
        Some(p) => {
            let id = args.id.unwrap_or(0);
            if id > 0 {
                secrets::set(id, p)?;
            }
            true
        }
        None => had_password,
    };
    let id = store::save_datasource(&conn, args.id, &args.input, password_saved)?;
    if let Some(p) = args.password.as_deref() {
        if args.id.is_none() {
            secrets::set(id, p)?;
            drop(conn);
            let conn = lock(&store)?;
            conn.execute(
                "UPDATE datasource SET password_saved = ?2 WHERE id = ?1",
                rusqlite::params![id, (!p.is_empty()) as i64],
            )
            .map_err(|e| e.to_string())?;
            return Ok(id);
        }
        if p.is_empty() {
            secrets::delete(id)?;
        }
    }
    Ok(id)
}

#[tauri::command]
fn delete_datasource(id: i64, store: State<'_, Store>) -> Result<(), String> {
    {
        let conn = lock(&store)?;
        store::delete_datasource(&conn, id)?;
    }
    secrets::delete(id)
}

/// Runs `SELECT 1` on the data source, so the editor can say whether it works.
#[tauri::command]
async fn test_datasource(id: i64, store: State<'_, Store>) -> Result<String, String> {
    let conn_str = connection_for(&store, id)?;
    let r = db::run_readonly(&conn_str, "SELECT version()", 10_000, 1).await?;
    Ok(r.rows
        .first()
        .and_then(|row| row.first().cloned().flatten())
        .unwrap_or_default())
}

/// Turns a `postgres://` URL or an `ssh://` string (the connection used before data sources) into a data source.
#[tauri::command]
fn import_connection(name: String, url: String, store: State<'_, Store>) -> Result<i64, String> {
    let url = url.trim();
    if remote::is_remote(url) {
        let t = remote::parse(url)?;
        let conn = lock(&store)?;
        return store::save_datasource(
            &conn,
            None,
            &store::DatasourceInput {
                name,
                host: String::new(),
                port: 5432,
                database: t.database,
                user: t.user,
                via_ssh: true,
                ssh_host: t.host,
                container: t.container,
            },
            false,
        );
    }
    let pg = connection::parse_pg_url(url)?;
    let id = {
        let conn = lock(&store)?;
        store::save_datasource(
            &conn,
            None,
            &store::DatasourceInput {
                name,
                host: pg.host,
                port: pg.port,
                database: pg.database,
                user: pg.user,
                via_ssh: false,
                ssh_host: String::new(),
                container: String::new(),
            },
            !pg.password.is_empty(),
        )?
    };
    if !pg.password.is_empty() {
        secrets::set(id, &pg.password)?;
    }
    Ok(id)
}

#[tauri::command]
fn query_datasource(query_id: String, store: State<'_, Store>) -> Result<Option<i64>, String> {
    let conn = lock(&store)?;
    store::query_datasource(&conn, &query_id)
}

#[tauri::command]
fn set_query_datasource(query_id: String, datasource_id: Option<i64>, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::set_query_datasource(&conn, &query_id, datasource_id)
}

#[tauri::command]
fn set_saved_datasource(id: i64, datasource_id: Option<i64>, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::set_saved_datasource(&conn, id, datasource_id)
}

/// A result as the table shows it, sent back for saving or export.
#[derive(Deserialize)]
struct ResultInput {
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
    truncated: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveResultArgs {
    name: String,
    source: String,
    datasource_id: Option<i64>,
    sql: String,
    result: ResultInput,
    /// The parameter values the result was made with, as JSON. Optional.
    #[serde(default)]
    params: Option<String>,
}

fn results_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("results");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Keeps a result: a CSV file in the results folder, and a record of it in the database.
#[tauri::command]
fn save_result(app: tauri::AppHandle, args: SaveResultArgs, store: State<'_, Store>) -> Result<i64, String> {
    let file = format!("result-{}.csv", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0));
    let text = csv::write(&args.result.columns, &args.result.rows);
    fs::write(results_dir(&app)?.join(&file), format!("\u{feff}{text}")).map_err(|e| e.to_string())?;
    let conn = lock(&store)?;
    store::insert_saved_result(
        &conn,
        &args.name,
        &args.source,
        args.datasource_id,
        &args.sql,
        args.result.rows.len() as i64,
        args.result.truncated,
        &file,
        args.params.as_deref(),
    )
}

#[tauri::command]
fn list_saved_results(store: State<'_, Store>) -> Result<Vec<store::SavedResult>, String> {
    let conn = lock(&store)?;
    store::list_saved_results(&conn)
}

/// A kept result, read back from its file, for the bottom panel.
#[tauri::command]
fn read_saved_result(app: tauri::AppHandle, id: i64, store: State<'_, Store>) -> Result<db::QueryResult, String> {
    let saved = {
        let conn = lock(&store)?;
        store::get_saved_result(&conn, id)?
    }
    .ok_or("the saved result no longer exists")?;
    let text = fs::read_to_string(results_dir(&app)?.join(&saved.file)).map_err(|e| e.to_string())?;
    let mut records = csv::parse(text.trim_start_matches('\u{feff}')).into_iter();
    let columns: Vec<String> = records
        .next()
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.unwrap_or_default())
        .collect();
    Ok(db::QueryResult {
        columns,
        rows: records.collect(),
        truncated: saved.truncated,
        elapsed_ms: 0,
        executed_sql: saved.sql,
    })
}

#[tauri::command]
fn delete_saved_result(app: tauri::AppHandle, id: i64, store: State<'_, Store>) -> Result<(), String> {
    let file = {
        let conn = lock(&store)?;
        let file = store::get_saved_result(&conn, id)?.map(|r| r.file);
        store::delete_saved_result(&conn, id)?;
        file
    };
    if let Some(file) = file {
        let _ = fs::remove_file(results_dir(&app)?.join(file));
    }
    Ok(())
}

/// Writes a result to a CSV file the user chose (UTF-8 with a byte-order mark, so Excel opens it as UTF-8).
#[tauri::command]
fn export_result_csv(path: String, result: ResultInput) -> Result<(), String> {
    let text = csv::write(&result.columns, &result.rows);
    fs::write(&path, format!("\u{feff}{text}")).map_err(|e| format!("{path}: {e}"))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupArgs {
    ssh_host: String,
    /// Folder on the server for the dump (`~` for the login user's home).
    remote_dir: String,
    remote_container: String,
    remote_db: String,
    local_container: String,
    local_db: String,
    /// The new data source's name, host port and login, for the restored database on this machine.
    datasource_name: String,
    local_port: i64,
    local_user: String,
    local_password: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupResult {
    dump_path: String,
    bytes: usize,
    local_db: String,
    datasource_id: i64,
    warnings: String,
}

/// Backs up a server database, restores it into a new local database and creates a data source for it.
#[tauri::command]
async fn back_up_database(
    app: tauri::AppHandle,
    args: BackupArgs,
    store: State<'_, Store>,
) -> Result<BackupResult, String> {
    let backups_dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("backups");
    let (ssh_host, remote_dir, remote_container, remote_db, local_container, local_db) = (
        args.ssh_host.clone(),
        args.remote_dir.clone(),
        args.remote_container.clone(),
        args.remote_db.clone(),
        args.local_container.clone(),
        args.local_db.clone(),
    );
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        backup::run(&backup::Plan {
            ssh_host: &ssh_host,
            remote_dir: &remote_dir,
            remote_container: &remote_container,
            remote_db: &remote_db,
            local_container: &local_container,
            local_db: &local_db,
            backups_dir,
        })
    })
    .await
    .map_err(|e| e.to_string())??;

    if !backup::name_ok(&args.local_db) {
        return Err("the local database name has characters that are not allowed".into());
    }
    let id = {
        let conn = lock(&store)?;
        store::save_datasource(
            &conn,
            None,
            &store::DatasourceInput {
                name: args.datasource_name.clone(),
                host: "localhost".into(),
                port: args.local_port,
                database: args.local_db.clone(),
                user: args.local_user.clone(),
                via_ssh: false,
                ssh_host: String::new(),
                container: String::new(),
            },
            !args.local_password.is_empty(),
        )?
    };
    if !args.local_password.is_empty() {
        secrets::set(id, &args.local_password)?;
    }
    Ok(BackupResult {
        dump_path: outcome.dump_path.to_string_lossy().to_string(),
        bytes: outcome.bytes,
        local_db: args.local_db,
        datasource_id: id,
        warnings: outcome.warnings,
    })
}

#[tauri::command]
fn list_saved_commands(store: State<'_, Store>) -> Result<Vec<store::SavedCommand>, String> {
    let conn = lock(&store)?;
    store::list_commands(&conn)
}

#[tauri::command]
fn list_command_groups(store: State<'_, Store>) -> Result<Vec<store::SavedGroup>, String> {
    let conn = lock(&store)?;
    store::list_command_groups(&conn)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveCommandArgs {
    id: Option<i64>,
    name: String,
    group: String,
    description: String,
    command: String,
    host: String,
}

#[tauri::command]
fn save_saved_command(args: SaveCommandArgs, store: State<'_, Store>) -> Result<i64, String> {
    let conn = lock(&store)?;
    store::save_command(&conn, args.id, &args.name, &args.group, &args.description, &args.command, &args.host)
}

#[tauri::command]
fn delete_saved_command(id: i64, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::delete_command(&conn, id)
}

#[tauri::command]
fn create_command_group(name: String, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::create_command_group(&conn, &name)
}

#[tauri::command]
fn delete_command_group(name: String, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::delete_command_group(&conn, &name)
}

#[tauri::command]
fn list_local_scripts(store: State<'_, Store>) -> Result<Vec<store::LocalScript>, String> {
    let conn = lock(&store)?;
    store::list_scripts(&conn)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveScriptArgs {
    id: Option<i64>,
    name: String,
    group: String,
    description: String,
    body: String,
}

#[tauri::command]
fn save_local_script(args: SaveScriptArgs, store: State<'_, Store>) -> Result<i64, String> {
    let conn = lock(&store)?;
    store::save_script(&conn, args.id, &args.name, &args.group, &args.description, &args.body)
}

#[tauri::command]
fn delete_local_script(id: i64, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::delete_script(&conn, id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunScriptArgs {
    id: i64,
    /// Values for the `:name` parameters. Missing or empty means an empty string.
    params: BTreeMap<String, Option<String>>,
}

/// Runs a saved script on this Mac. Its parameters are filled in like a server command's. The output is a
/// result, like a server command's.
#[tauri::command]
async fn run_local_script(args: RunScriptArgs, store: State<'_, Store>) -> Result<db::QueryResult, String> {
    let saved = {
        let conn = lock(&store)?;
        store::list_scripts(&conn)?
            .into_iter()
            .find(|s| s.id == args.id)
            .ok_or_else(|| format!("local script {} not found", args.id))?
    };
    let mut values: BTreeMap<String, Option<String>> = server_commands::used_params(&saved.body)
        .into_iter()
        .map(|name| (name, None))
        .collect();
    for (name, v) in args.params {
        values.insert(name, v.filter(|s| !s.is_empty()));
    }
    let script = server_commands::render(&saved.body, &values);
    tauri::async_runtime::spawn_blocking(move || local_script::run(&script, local_script::TIMEOUT))
        .await
        .map_err(|e| e.to_string())?
}

/// Writes every saved script as a `.sh` file into the folder. Returns how many were written.
#[tauri::command]
async fn export_local_scripts(dir: String, store: State<'_, Store>) -> Result<usize, String> {
    let scripts = {
        let conn = lock(&store)?;
        store::list_scripts(&conn)?
    };
    tauri::async_runtime::spawn_blocking(move || local_script::export_all(&dir, &scripts))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RunCommandArgs {
    id: i64,
    /// The ssh alias to run on.
    host: String,
    /// Values for the `:name` parameters. Missing or empty means an empty string.
    params: BTreeMap<String, Option<String>>,
}

/// Runs a saved command on a server and returns its output as a result (the bottom panel, export, keep).
#[tauri::command]
async fn run_saved_command(args: RunCommandArgs, store: State<'_, Store>) -> Result<db::QueryResult, String> {
    let saved = {
        let conn = lock(&store)?;
        store::list_commands(&conn)?
            .into_iter()
            .find(|c| c.id == args.id)
            .ok_or_else(|| format!("saved command {} not found", args.id))?
    };
    let mut values: BTreeMap<String, Option<String>> = server_commands::used_params(&saved.command)
        .into_iter()
        .map(|name| (name, None))
        .collect();
    for (name, v) in args.params {
        values.insert(name, v.filter(|s| !s.is_empty()));
    }
    let rendered = server_commands::render(&saved.command, &values);
    let host = args.host.trim().to_string();
    let rendered_for_run = rendered.clone();
    let outcome = tauri::async_runtime::spawn_blocking(move || server_commands::run(&host, &rendered_for_run, 5_000))
        .await
        .map_err(|e| e.to_string())?;

    let params_json = serde_json::to_string(&values).unwrap_or_else(|_| "{}".to_string());
    let query_id = format!("command:{}", saved.id);
    if let Ok(conn) = lock(&store) {
        let (ok, rows, elapsed, error) = match &outcome {
            Ok(r) => (true, Some(r.rows.len() as i64), Some(r.elapsed_ms as i64), None),
            Err(e) => (false, None, None, Some(e.as_str())),
        };
        let _ = store::record_run(
            &conn,
            &RunInput {
                sql: &rendered,
                query_id: &query_id,
                path: "",
                description: &saved.name,
                params_json: &params_json,
                ok,
                rows,
                elapsed_ms: elapsed,
                error,
            },
        );
    }
    outcome
}

/// Indexes the Markdown documents under the knowledge folder (repo_quizbuks).
#[tauri::command]
async fn index_knowledge(dir: String, store: State<'_, Store>) -> Result<knowledge::IndexReport, String> {
    let mut conn = lock(&store)?;
    knowledge::index(&mut conn, Path::new(&dir))
}

/// The documents that fit the result's terms, within the size budget (bytes).
#[tauri::command]
fn select_knowledge(text: String, budget_bytes: i64, store: State<'_, Store>) -> Result<Vec<knowledge::Hit>, String> {
    let conn = lock(&store)?;
    knowledge::select(&conn, &text, budget_bytes)
}

#[tauri::command]
fn read_knowledge(path: String, store: State<'_, Store>) -> Result<String, String> {
    let conn = lock(&store)?;
    knowledge::read(&conn, &path)
}

/// Stores the AI service's API key in the keychain. The key is never returned to the frontend.
#[tauri::command]
fn save_ai_key(key: String) -> Result<(), String> {
    let key = key.trim();
    if key.is_empty() {
        return Err("the key is empty".into());
    }
    secrets::set_named(ai::KEY_NAME, key)
}

#[tauri::command]
fn ai_key_saved() -> Result<bool, String> {
    Ok(secrets::get_named(ai::KEY_NAME)?.is_some())
}

#[tauri::command]
fn remove_ai_key() -> Result<(), String> {
    secrets::delete_named(ai::KEY_NAME)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareArgs {
    #[serde(flatten)]
    input: ai::Input,
}

/// Builds the request for the user to see. Nothing is sent by this command.
#[tauri::command]
fn prepare_analysis(args: PrepareArgs, store: State<'_, Store>) -> Result<ai::Prepared, String> {
    let conn = lock(&store)?;
    ai::prepare(&conn, &args.input)
}

/// Suggests the documents for a result: by the terms in its SQL, description and columns, within a budget.
#[tauri::command]
fn suggest_documents(text: String, budget_bytes: i64, store: State<'_, Store>) -> Result<Vec<knowledge::Hit>, String> {
    let conn = lock(&store)?;
    knowledge::select(&conn, &text, budget_bytes)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendArgs {
    system: String,
    user: String,
    model: String,
}

/// Sends the request the user confirmed, and returns the answer.
#[tauri::command]
async fn send_analysis(args: SendArgs) -> Result<String, String> {
    ai::send(&args.system, &args.user, &args.model).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveAnalysisArgs {
    id: Option<i64>,
    name: String,
    source: String,
    request: String,
    answer: String,
    model: String,
    /// The saved result the analysis was made from.
    result_id: Option<i64>,
}

#[tauri::command]
fn save_analysis(args: SaveAnalysisArgs, store: State<'_, Store>) -> Result<i64, String> {
    let conn = lock(&store)?;
    store::save_analysis(
        &conn,
        args.id,
        &args.name,
        &args.source,
        &args.request,
        &args.answer,
        &args.model,
        args.result_id,
    )
}

#[tauri::command]
fn list_saved_analyses(store: State<'_, Store>) -> Result<Vec<store::SavedAnalysis>, String> {
    let conn = lock(&store)?;
    store::list_analyses(&conn)
}

#[tauri::command]
fn get_saved_analysis(id: i64, store: State<'_, Store>) -> Result<store::SavedAnalysis, String> {
    let conn = lock(&store)?;
    store::get_analysis(&conn, id)?.ok_or_else(|| "the analysis no longer exists".into())
}

#[tauri::command]
fn delete_saved_analysis(id: i64, store: State<'_, Store>) -> Result<(), String> {
    let conn = lock(&store)?;
    store::delete_analysis(&conn, id)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenerateArgs {
    #[serde(flatten)]
    input: generate::Input,
    mapping_path: Option<String>,
}

/// The request for generating a statement or a command. Nothing is sent by this command.
#[tauri::command]
fn prepare_generation(args: GenerateArgs, store: State<'_, Store>) -> Result<generate::Prepared, String> {
    let mapping = load_mapping(args.mapping_path.as_deref())?;
    let conn = lock(&store)?;
    generate::prepare(&conn, &mapping, &args.input)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendGenerationArgs {
    kind: String,
    system: String,
    user: String,
    model: String,
}

/// Sends the request the user confirmed, and returns the checked proposal.
#[tauri::command]
async fn send_generation(args: SendGenerationArgs) -> Result<generate::Proposal, String> {
    let answer = ai::send(&args.system, &args.user, &args.model).await?;
    generate::proposal(&args.kind, &answer)
}

#[derive(serde::Serialize)]
struct TableColumn {
    field: String,
    column: String,
}

#[derive(serde::Serialize)]
struct TableInfo {
    entity: String,
    table: String,
    temporal: bool,
    /// The primary key column of a versioned table.
    pk: Option<String>,
    /// The system's logical version columns of a time-versioned table (e.g. versionNo), if any.
    columns: Vec<TableColumn>,
}

/// Every table of the mapping, with its columns (the fields of its entity). From the mapping file.
#[tauri::command]
fn list_tables(mapping_path: Option<String>) -> Result<Vec<TableInfo>, String> {
    let mapping = load_mapping(mapping_path.as_deref())?;
    let mut out = Vec::new();
    for (entity, table) in &mapping.tables {
        if entity.contains('.') {
            continue;
        }
        let prefix = format!("{entity}.");
        let columns: Vec<TableColumn> = mapping
            .tables
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&prefix).map(|f| TableColumn { field: f.to_string(), column: v.clone() })
            })
            .collect();
        out.push(TableInfo { entity: entity.clone(), table: table.clone(), temporal: false, pk: None, columns });
    }
    for (entity, t) in &mapping.temporal {
        let mut columns: Vec<TableColumn> = mapping
            .tables
            .iter()
            .filter_map(|(k, v)| {
                k.strip_prefix(&format!("{entity}.")).map(|f| TableColumn { field: f.to_string(), column: v.clone() })
            })
            .collect();
        columns.extend(t.columns.iter().map(|(f, c)| TableColumn { field: f.clone(), column: c.clone() }));
        out.push(TableInfo { entity: entity.clone(), table: t.table.clone(), temporal: true, pk: t.pk.clone(), columns });
    }
    out.sort_by(|a, b| a.table.cmp(&b.table));
    Ok(out)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectTableArgs {
    table: String,
    /// The most rows to return; `None` returns every row (up to a safety cap).
    limit: Option<i64>,
    /// The columns to return, as their physical names; `None` returns all of them.
    #[serde(default)]
    columns: Option<Vec<String>>,
    /// The data source to run on; the default when the editor has one.
    datasource_id: Option<i64>,
}

/// Rows returned when the user asks for every row: a safety cap, shown as truncated when reached.
const ALL_ROWS_CAP: usize = 100_000;

/// `SELECT * FROM table [LIMIT n]` on the data source, read-only. The table name is checked first, so it
/// can only be a plain name of the mapping.
#[tauri::command]
async fn select_table(args: SelectTableArgs, store: State<'_, Store>) -> Result<db::QueryResult, String> {
    let table = args.table.trim();
    if table.is_empty() || !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("not a table name: {table}"));
    }
    let ds = args.datasource_id.ok_or("choose a default data source in Settings first")?;
    let list = match &args.columns {
        Some(cols) => versioned::column_list(cols)?,
        None => "*".to_string(),
    };
    let (sql, max_rows, timeout) = match args.limit {
        Some(n) if n > 0 => (format!("SELECT {list} FROM {table} LIMIT {n}"), n as usize, 30_000),
        Some(_) => return Err("the row limit must be at least 1".into()),
        None => (format!("SELECT {list} FROM {table}"), ALL_ROWS_CAP, 120_000),
    };
    let conn_str = connection_for(&store, ds)?;
    db::run_readonly(&conn_str, &sql, timeout, max_rows).await
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionedArgs {
    mapping_path: Option<String>,
    table: String,
    /// The query: current, history, asOf, changes or deleted.
    query: String,
    params: BTreeMap<String, String>,
    /// The columns to return, as their physical names; `None` returns all of them.
    #[serde(default)]
    columns: Option<Vec<String>>,
    datasource_id: Option<i64>,
}

/// The versioned queries a table allows, with their parameters. Empty for a table that is not versioned.
#[tauri::command]
fn versioned_queries(mapping_path: Option<String>, table: String) -> Result<Vec<versioned::QueryDef>, String> {
    let tables = list_tables(mapping_path)?;
    let info = tables.iter().find(|t| t.table == table);
    Ok(match info {
        Some(t) if t.temporal => versioned::definitions(),
        _ => Vec::new(),
    })
}

/// Runs one versioned query on the data source, read-only, and returns its rows.
#[tauri::command]
async fn run_versioned_query(args: VersionedArgs, store: State<'_, Store>) -> Result<(db::QueryResult, String), String> {
    let tables = list_tables(args.mapping_path.clone())?;
    let info = tables
        .iter()
        .find(|t| t.table == args.table && t.temporal)
        .ok_or("this table is not a versioned table")?;
    let column = |field: &str| -> Result<String, String> {
        info.columns
            .iter()
            .find(|c| c.field == field)
            .map(|c| c.column.clone())
            .ok_or_else(|| format!("the table has no {field} column in the mapping"))
    };
    let pk = info.pk.clone().ok_or("the table has no primary key in the mapping")?;
    let vcols = (column("versionNo")?, column("effectStartTime")?, column("createdTime")?, column("deleted")?);
    let v = versioned::Versioned {
        table: &info.table,
        pk: &pk,
        version_no: &vcols.0,
        effect_start: &vcols.1,
        created: &vcols.2,
        deleted: &vcols.3,
    };
    let sql = versioned::build(&v, &args.query, &args.params, args.columns.as_deref())?;
    let ds = args.datasource_id.ok_or("choose a default data source in Settings first")?;
    let conn_str = connection_for(&store, ds)?;
    let r = db::run_readonly(&conn_str, &sql, 60_000, 100_000).await?;
    Ok((r, sql))
}

/// The tables that exist in the data source's database (outside the system schemas). The mapping lists
/// the tables of the code, which a database may not have yet, so the app checks against this.
#[tauri::command]
async fn existing_tables(datasource_id: Option<i64>, store: State<'_, Store>) -> Result<Vec<String>, String> {
    let ds = datasource_id.ok_or("choose a default data source in Settings first")?;
    let conn_str = connection_for(&store, ds)?;
    let r = db::run_readonly(
        &conn_str,
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema NOT IN ('pg_catalog', 'information_schema') AND table_type = 'BASE TABLE' ORDER BY 1",
        30_000,
        100_000,
    )
    .await?;
    Ok(r.rows.into_iter().filter_map(|row| row.into_iter().next().flatten()).collect())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Entry point of the `sqlpool-mcp` binary (a read-only MCP server for Claude Desktop). Returns the exit code.
pub fn mcp_main(args: &[String]) -> i32 {
    mcp::main_with_args(args)
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            fs::create_dir_all(&dir)?;
            let conn = store::open(&dir.join("sqlpool.db"))?;
            app.manage(Store(Mutex::new(conn)));

            // macOS app menu: About, Settings (⌘,). Edit menu so ⌘C / ⌘V work in text fields.
            let handle = app.handle();
            let about = PredefinedMenuItem::about(
                handle,
                Some("About sqlpool"),
                Some(AboutMetadata {
                    name: Some("sqlpool".into()),
                    version: Some(env!("CARGO_PKG_VERSION").into()),
                    ..Default::default()
                }),
            )?;
            let settings = MenuItem::with_id(handle, "settings", "Settings…", true, Some("CmdOrCtrl+,"))?;
            let sep = PredefinedMenuItem::separator(handle)?;
            let app_menu = Submenu::with_items(
                handle,
                "sqlpool",
                true,
                &[
                    &about,
                    &sep,
                    &settings,
                    &sep,
                    &PredefinedMenuItem::hide(handle, None)?,
                    &PredefinedMenuItem::quit(handle, None)?,
                ],
            )?;
            let edit_menu = Submenu::with_items(
                handle,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(handle, None)?,
                    &PredefinedMenuItem::redo(handle, None)?,
                    &sep,
                    &PredefinedMenuItem::cut(handle, None)?,
                    &PredefinedMenuItem::copy(handle, None)?,
                    &PredefinedMenuItem::paste(handle, None)?,
                    &PredefinedMenuItem::select_all(handle, None)?,
                ],
            )?;
            let menu = Menu::with_items(handle, &[&app_menu, &edit_menu])?;
            handle.set_menu(menu)?;
            handle.on_menu_event(|app, event| {
                if event.id().as_ref() == "settings" {
                    let _ = app.emit("open-settings", ());
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sync_catalog,
            search_queries,
            set_query_meta,
            query_history,
            get_query,
            run_query,
            find_query,
            list_saved_sql,
            save_saved_sql,
            save_catalog_as,
            delete_saved_sql,
            list_saved_groups,
            create_saved_group,
            delete_saved_group,
            recent_runs,
            run_saved_sql,
            list_datasources,
            save_datasource,
            delete_datasource,
            test_datasource,
            import_connection,
            query_datasource,
            set_query_datasource,
            set_saved_datasource,
            save_result,
            list_saved_results,
            read_saved_result,
            delete_saved_result,
            export_result_csv,
            back_up_database,
            list_saved_commands,
            list_command_groups,
            save_saved_command,
            delete_saved_command,
            create_command_group,
            delete_command_group,
            list_local_scripts,
            save_local_script,
            delete_local_script,
            run_local_script,
            export_local_scripts,
            run_saved_command,
            index_knowledge,
            save_ai_key,
            ai_key_saved,
            remove_ai_key,
            prepare_analysis,
            suggest_documents,
            send_analysis,
            save_analysis,
            list_saved_analyses,
            get_saved_analysis,
            delete_saved_analysis,
            prepare_generation,
            list_tables,
            select_table,
            existing_tables,
            versioned_queries,
            run_versioned_query,
            send_generation,
            select_knowledge,
            read_knowledge,
            list_environments,
            save_environment,
            rename_environment,
            delete_environment
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
