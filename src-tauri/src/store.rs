//! Local SQLite store: search index over the catalog, favorites, tags and run history.
//!
//! The `.sql` files stay the source of truth. This store only holds what the
//! app adds on top: the search index (FTS5), favorites, tags, and every run.

use crate::catalog;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

/// Tauri managed state. One connection, guarded by a mutex; never held across `.await`.
pub struct Store(pub Mutex<Connection>);

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS query_meta (
    query_id     TEXT PRIMARY KEY,
    path         TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    tags         TEXT NOT NULL DEFAULT '',
    favorite     INTEGER NOT NULL DEFAULT 0,
    last_run_at  INTEGER,
    run_count    INTEGER NOT NULL DEFAULT 0
);
CREATE VIRTUAL TABLE IF NOT EXISTS query_fts USING fts5(
    query_id UNINDEXED, description, tags, body, tokenize = 'trigram'
);
CREATE TABLE IF NOT EXISTS run_history (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    query_id    TEXT NOT NULL,
    params_json TEXT NOT NULL,
    ran_at      INTEGER NOT NULL,
    ok          INTEGER NOT NULL,
    rows        INTEGER,
    elapsed_ms  INTEGER,
    error       TEXT
);
CREATE INDEX IF NOT EXISTS idx_run_history_query ON run_history(query_id, id);
CREATE TABLE IF NOT EXISTS datasource (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    name           TEXT NOT NULL UNIQUE,
    host           TEXT NOT NULL DEFAULT '',
    port           INTEGER NOT NULL DEFAULT 5432,
    database       TEXT NOT NULL DEFAULT '',
    user           TEXT NOT NULL DEFAULT '',
    via_ssh        INTEGER NOT NULL DEFAULT 0,
    ssh_host       TEXT NOT NULL DEFAULT '',
    container      TEXT NOT NULL DEFAULT '',
    password_saved INTEGER NOT NULL DEFAULT 0,
    updated_at     INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS saved_analysis (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL,
    source     TEXT NOT NULL DEFAULT '',
    request    TEXT NOT NULL,
    answer     TEXT NOT NULL,
    model      TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    result_id  INTEGER
);
CREATE TABLE IF NOT EXISTS knowledge_doc (
    path  TEXT PRIMARY KEY,
    title TEXT NOT NULL,
    bytes INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    body  TEXT NOT NULL
);
CREATE VIRTUAL TABLE IF NOT EXISTS knowledge_fts USING fts5(
    path UNINDEXED, title, body, tokenize = 'trigram'
);
CREATE TABLE IF NOT EXISTS saved_command (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    grp         TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    command     TEXT NOT NULL,
    host        TEXT NOT NULL DEFAULT '',
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS command_group (
    name       TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS local_script (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    grp         TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    body        TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS saved_result (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT NOT NULL,
    source        TEXT NOT NULL DEFAULT '',
    datasource_id INTEGER,
    sql           TEXT NOT NULL,
    row_count     INTEGER NOT NULL,
    truncated     INTEGER NOT NULL DEFAULT 0,
    created_at    INTEGER NOT NULL,
    file          TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS saved_group (
    name       TEXT PRIMARY KEY,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS saved_sql (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    sql         TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS environment (
    name        TEXT PRIMARY KEY,
    parent      TEXT,
    description TEXT NOT NULL DEFAULT '',
    values_json TEXT NOT NULL DEFAULT '{}',
    updated_at  INTEGER NOT NULL
);
";

/// Keep at most this many runs per query.
const HISTORY_KEEP: i64 = 200;

#[derive(Debug, Serialize)]
pub struct QueryHit {
    pub query_id: String,
    pub path: String,
    pub description: String,
    pub tags: String,
    pub favorite: bool,
    pub last_run_at: Option<i64>,
    pub run_count: i64,
}

#[derive(Debug, Serialize)]
pub struct RunRecord {
    pub id: i64,
    pub query_id: String,
    pub params_json: String,
    pub ran_at: i64,
    pub ok: bool,
    pub rows: Option<i64>,
    pub elapsed_ms: Option<i64>,
    pub error: Option<String>,
}

/// A SQL statement the user typed and saved. Runs of it are recorded under query_id "saved:<id>".
#[derive(Debug, Serialize)]
pub struct SavedSql {
    pub id: i64,
    /// The data source the statement runs on; `None` means the environment's or the default one.
    pub datasource_id: Option<i64>,
    pub name: String,
    /// Group the statement is filed under; empty means ungrouped.
    pub group: String,
    pub description: String,
    pub sql: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// A group (folder) for saved statements. Groups exist on their own, even with no statements in them.
#[derive(Debug, Serialize)]
pub struct SavedGroup {
    pub name: String,
    pub count: i64,
}

pub fn list_groups(conn: &Connection) -> Result<Vec<SavedGroup>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT g.name, (SELECT COUNT(*) FROM saved_sql s WHERE s.grp = g.name) AS n
             FROM saved_group g
             UNION
             SELECT grp, COUNT(*) FROM saved_sql WHERE grp <> '' GROUP BY grp
             ORDER BY 1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok(SavedGroup { name: r.get(0)?, count: r.get(1)? }))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn create_group(conn: &Connection, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("group name is required".into());
    }
    conn.execute(
        "INSERT OR IGNORE INTO saved_group (name, created_at) VALUES (?1, ?2)",
        params![name, now_ms()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Deletes the group. Its statements are kept and moved to Ungrouped.
pub fn delete_group(conn: &Connection, name: &str) -> Result<(), String> {
    conn.execute("UPDATE saved_sql SET grp = '' WHERE grp = ?1", params![name])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM saved_group WHERE name = ?1", params![name])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A named set of parameter values. It may have a parent: the values of the parent apply
/// unless this environment sets the same parameter. Forms fill from the environment in use.
#[derive(Debug, Serialize)]
pub struct Environment {
    pub name: String,
    pub parent: Option<String>,
    /// The data source this environment uses by default (its own, not inherited).
    pub datasource_id: Option<i64>,
    pub description: String,
    /// Parameter name -> value this environment sets itself (text as typed in a form).
    pub values: std::collections::BTreeMap<String, String>,
}

type Values = std::collections::BTreeMap<String, String>;

pub fn list_environments(conn: &Connection) -> Result<Vec<Environment>, String> {
    let mut stmt = conn
        .prepare("SELECT name, parent, description, values_json, datasource_id FROM environment ORDER BY name COLLATE NOCASE")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        let (name, parent, description, json, datasource_id) = row.map_err(|e| e.to_string())?;
        let values: Values = serde_json::from_str(&json).map_err(|e| format!("environment {name}: {e}"))?;
        out.push(Environment { name, parent, datasource_id, description, values });
    }
    Ok(out)
}

fn parent_of(conn: &Connection, name: &str) -> Result<Option<String>, String> {
    conn.query_row("SELECT parent FROM environment WHERE name = ?1", params![name], |r| r.get(0))
        .optional()
        .map(|v| v.flatten())
        .map_err(|e| e.to_string())
}

/// Creates or replaces an environment. Empty values are dropped (they mean "not set").
/// The parent must exist, and may not be the environment itself or one of its descendants.
pub fn save_environment(
    conn: &Connection,
    name: &str,
    parent: Option<&str>,
    description: &str,
    values: &Values,
    datasource_id: Option<i64>,
) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("environment name is required".into());
    }
    let parent = parent.map(str::trim).filter(|p| !p.is_empty());
    if let Some(p) = parent {
        if p == name {
            return Err("an environment cannot be its own parent".into());
        }
        let exists: bool = conn
            .query_row("SELECT COUNT(*) FROM environment WHERE name = ?1", params![p], |r| r.get::<_, i64>(0))
            .map(|n| n > 0)
            .map_err(|e| e.to_string())?;
        if !exists {
            return Err(format!("parent environment {p} does not exist"));
        }
        // Walk up from the parent: meeting `name` again would make a cycle.
        let mut cur = Some(p.to_string());
        let mut steps = 0;
        while let Some(c) = cur {
            if c == name {
                return Err("the parent cannot be a child of this environment".into());
            }
            steps += 1;
            if steps > 1000 {
                return Err("environment tree is too deep".into());
            }
            cur = parent_of(conn, &c)?;
        }
    }
    let kept: std::collections::BTreeMap<&String, &String> =
        values.iter().filter(|(k, v)| !k.trim().is_empty() && !v.is_empty()).collect();
    let json = serde_json::to_string(&kept).map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO environment (name, parent, description, values_json, updated_at, datasource_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(name) DO UPDATE SET parent = excluded.parent, description = excluded.description,
            values_json = excluded.values_json, updated_at = excluded.updated_at, datasource_id = excluded.datasource_id",
        params![name, parent, description.trim(), json, now_ms(), datasource_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Renames an environment and points its children at the new name.
pub fn rename_environment(conn: &Connection, old: &str, new: &str) -> Result<(), String> {
    let new = new.trim();
    if new.is_empty() {
        return Err("environment name is required".into());
    }
    if old == new {
        return Ok(());
    }
    let taken: i64 = conn
        .query_row("SELECT COUNT(*) FROM environment WHERE name = ?1", params![new], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    if taken > 0 {
        return Err(format!("an environment named {new} already exists"));
    }
    conn.execute("UPDATE environment SET parent = ?2 WHERE parent = ?1", params![old, new])
        .map_err(|e| e.to_string())?;
    conn.execute("UPDATE environment SET name = ?2 WHERE name = ?1", params![old, new])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Deletes an environment. Its children stay, at the top of the tree.
pub fn delete_environment(conn: &Connection, name: &str) -> Result<(), String> {
    conn.execute("UPDATE environment SET parent = NULL WHERE parent = ?1", params![name])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM environment WHERE name = ?1", params![name])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct SyncReport {
    pub indexed: usize,
    /// Files that could not be read or parsed, or that repeat an id: "path: reason".
    pub skipped: Vec<String>,
}

pub fn open(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    // Saved SQL got a group column later; add it to databases created before.
    let has_group = conn
        .prepare("SELECT 1 FROM pragma_table_info('saved_sql') WHERE name = 'grp'")
        .map_err(|e| e.to_string())?
        .exists([])
        .map_err(|e| e.to_string())?;
    if !has_group {
        conn.execute_batch("ALTER TABLE saved_sql ADD COLUMN grp TEXT NOT NULL DEFAULT ''")
            .map_err(|e| e.to_string())?;
    }
    add_column(conn, "saved_sql", "datasource_id", "INTEGER")?;
    add_column(conn, "saved_result", "params", "TEXT")?;
    add_column(conn, "saved_analysis", "result_id", "INTEGER")?;
    seed_commands(conn)?;
    // Local scripts used to take variables from a text box; they now take the environment's values.
    let has_vars = conn
        .prepare("SELECT 1 FROM pragma_table_info('local_script') WHERE name = 'vars'")
        .map_err(|e| e.to_string())?
        .exists([])
        .map_err(|e| e.to_string())?;
    if has_vars {
        conn.execute_batch("ALTER TABLE local_script DROP COLUMN vars").map_err(|e| e.to_string())?;
    }
    add_column(conn, "environment", "datasource_id", "INTEGER")?;
    add_column(conn, "run_history", "sql", "TEXT NOT NULL DEFAULT ''")?;
    add_column(conn, "query_meta", "datasource_id", "INTEGER")?;
    // Environments got a parent and a description after the table existed.
    for (col, ddl) in [
        ("parent", "ALTER TABLE environment ADD COLUMN parent TEXT"),
        ("description", "ALTER TABLE environment ADD COLUMN description TEXT NOT NULL DEFAULT ''"),
    ] {
        let has: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('environment') WHERE name = ?1",
                params![col],
                |r| r.get::<_, i64>(0),
            )
            .map(|n| n > 0)
            .map_err(|e| e.to_string())?;
        if !has {
            conn.execute_batch(ddl).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// The mail check that was built in as a fixed command, saved once as an ordinary saved command.
fn seed_commands(conn: &Connection) -> Result<(), String> {
    let has: bool = conn
        .query_row("SELECT COUNT(*) FROM saved_command WHERE name = ?1", params!["Mail settings in the app container"], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n > 0)
        .map_err(|e| e.to_string())?;
    if !has {
        save_command(
            conn,
            None,
            "Mail settings in the app container",
            "",
            "The SPRING_MAIL and JABIZ_MAIL variables of quizbuks-staging-app. The password only shows its length.",
            r#"docker exec quizbuks-staging-app sh -c 'env | grep -E "^(SPRING_MAIL|JABIZ_MAIL)" | sed -E "s/^(SPRING_MAIL_PASSWORD=).*/\1(length: $(printf %s "$SPRING_MAIL_PASSWORD" | wc -c))/"'"#,
            "quizbuks-staging",
        )?;
    }
    Ok(())
}

/// Adds a column to a table created before it existed.
fn add_column(conn: &Connection, table: &str, column: &str, ddl: &str) -> Result<(), String> {
    let has: bool = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            params![table, column],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .map_err(|e| e.to_string())?;
    if !has {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {ddl}"))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Creates or refreshes the metadata row for a query (keeps favorite, tags, history).
fn ensure_meta(conn: &Connection, query_id: &str, path: &str, description: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO query_meta (query_id, path, description) VALUES (?1, ?2, ?3)
         ON CONFLICT(query_id) DO UPDATE SET path = excluded.path, description = excluded.description",
        params![query_id, path, description],
    )?;
    Ok(())
}

/// Re-reads the catalog directory and rebuilds the search index.
/// Favorites, tags and history are kept even for queries whose file is gone.
pub fn sync_catalog(conn: &mut Connection, dir: &Path) -> Result<SyncReport, String> {
    let files = catalog::collect_sql_files(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM query_fts", []).map_err(|e| e.to_string())?;

    let mut indexed = 0;
    let mut skipped = Vec::new();
    let mut seen = HashSet::new();

    for path in files {
        let display = path.display().to_string();
        let source = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                skipped.push(format!("{display}: {e}"));
                continue;
            }
        };
        let (front, body) = match catalog::parse_source(&source) {
            Ok(v) => v,
            Err(e) => {
                skipped.push(format!("{display}: {e}"));
                continue;
            }
        };
        if !seen.insert(front.id.clone()) {
            skipped.push(format!("{display}: duplicate id {}", front.id));
            continue;
        }

        ensure_meta(&tx, &front.id, &display, &front.description).map_err(|e| e.to_string())?;
        let tags: String = tx
            .query_row(
                "SELECT tags FROM query_meta WHERE query_id = ?1",
                params![front.id],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO query_fts (query_id, description, tags, body) VALUES (?1, ?2, ?3, ?4)",
            params![front.id, front.description, tags, body],
        )
        .map_err(|e| e.to_string())?;
        indexed += 1;
    }

    tx.commit().map_err(|e| e.to_string())?;
    Ok(SyncReport { indexed, skipped })
}

const HIT_COLUMNS: &str = "SELECT m.query_id, m.path, m.description, m.tags, m.favorite, m.last_run_at, m.run_count
     FROM query_meta m WHERE ";

/// Search in the index. Empty query lists everything that has a file on disk.
/// Terms of 3+ characters use the trigram FTS index (substring match, works for CJK).
/// Shorter terms fall back to LIKE on description, tags and body.
pub fn search(conn: &Connection, text: &str) -> Result<Vec<QueryHit>, String> {
    let tokens: Vec<&str> = text.split_whitespace().collect();

    let (condition, values): (String, Vec<String>) = if tokens.is_empty() {
        ("m.query_id IN (SELECT query_id FROM query_fts)".to_string(), vec![])
    } else if tokens.iter().all(|t| t.chars().count() >= 3) {
        let expr = tokens
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(" ");
        (
            "m.query_id IN (SELECT query_id FROM query_fts WHERE query_fts MATCH ?1)".to_string(),
            vec![expr],
        )
    } else {
        let mut parts = Vec::new();
        let mut values = Vec::new();
        for t in &tokens {
            let n = values.len() + 1;
            parts.push(format!(
                "m.query_id IN (SELECT query_id FROM query_fts WHERE description LIKE ?{n} OR tags LIKE ?{n} OR body LIKE ?{n})"
            ));
            values.push(format!("%{t}%"));
        }
        (parts.join(" AND "), values)
    };

    let sql = format!("{HIT_COLUMNS}{condition} ORDER BY m.favorite DESC, m.query_id");
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params_from_iter(values.iter()), |r| {
            Ok(QueryHit {
                query_id: r.get(0)?,
                path: r.get(1)?,
                description: r.get(2)?,
                tags: r.get(3)?,
                favorite: r.get::<_, i64>(4)? != 0,
                last_run_at: r.get(5)?,
                run_count: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn set_meta(
    conn: &Connection,
    query_id: &str,
    favorite: Option<bool>,
    tags: Option<&str>,
) -> Result<(), String> {
    if let Some(f) = favorite {
        conn.execute(
            "UPDATE query_meta SET favorite = ?2 WHERE query_id = ?1",
            params![query_id, f as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    if let Some(t) = tags {
        let t = t.trim();
        conn.execute(
            "UPDATE query_meta SET tags = ?2 WHERE query_id = ?1",
            params![query_id, t],
        )
        .map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE query_fts SET tags = ?2 WHERE query_id = ?1",
            params![query_id, t],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub struct RunInput<'a> {
    /// The SQL as it was sent to the database, parameters rendered.
    pub sql: &'a str,
    pub query_id: &'a str,
    pub path: &'a str,
    pub description: &'a str,
    pub params_json: &'a str,
    pub ok: bool,
    pub rows: Option<i64>,
    pub elapsed_ms: Option<i64>,
    pub error: Option<&'a str>,
}

pub fn record_run(conn: &Connection, run: &RunInput) -> Result<(), String> {
    let ran_at = now_ms();
    ensure_meta(conn, run.query_id, run.path, run.description).map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE query_meta SET last_run_at = ?2, run_count = run_count + 1 WHERE query_id = ?1",
        params![run.query_id, ran_at],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO run_history (query_id, params_json, ran_at, ok, rows, elapsed_ms, error, sql)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            run.query_id,
            run.params_json,
            ran_at,
            run.ok as i64,
            run.rows,
            run.elapsed_ms,
            run.error,
            run.sql
        ],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "DELETE FROM run_history WHERE query_id = ?1 AND id NOT IN (
             SELECT id FROM run_history WHERE query_id = ?1 ORDER BY id DESC LIMIT ?2)",
        params![run.query_id, HISTORY_KEEP],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn history(conn: &Connection, query_id: &str, limit: i64) -> Result<Vec<RunRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, query_id, params_json, ran_at, ok, rows, elapsed_ms, error
             FROM run_history WHERE query_id = ?1 ORDER BY id DESC LIMIT ?2",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![query_id, limit], |r| {
            Ok(RunRecord {
                id: r.get(0)?,
                query_id: r.get(1)?,
                params_json: r.get(2)?,
                ran_at: r.get(3)?,
                ok: r.get::<_, i64>(4)? != 0,
                rows: r.get(5)?,
                elapsed_ms: r.get(6)?,
                error: r.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn list_saved(conn: &Connection) -> Result<Vec<SavedSql>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, grp, description, sql, created_at, updated_at, datasource_id FROM saved_sql ORDER BY grp, updated_at DESC, id DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SavedSql {
                id: r.get(0)?,
                datasource_id: r.get(7)?,
                name: r.get(1)?,
                group: r.get(2)?,
                description: r.get(3)?,
                sql: r.get(4)?,
                created_at: r.get(5)?,
                updated_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Inserts a new statement (id None) or updates an existing one. Returns its id.
pub fn save_sql(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    group: &str,
    description: &str,
    sql: &str,
) -> Result<i64, String> {
    let name = name.trim();
    let group = group.trim();
    if name.is_empty() {
        return Err("name is required".into());
    }
    if sql.trim().is_empty() {
        return Err("SQL is empty".into());
    }
    let now = now_ms();
    match id {
        Some(id) => {
            let changed = conn
                .execute(
                    "UPDATE saved_sql SET name = ?2, grp = ?3, description = ?4, sql = ?5, updated_at = ?6 WHERE id = ?1",
                    params![id, name, group, description.trim(), sql, now],
                )
                .map_err(|e| e.to_string())?;
            if changed == 0 {
                return Err(format!("saved SQL {id} not found"));
            }
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO saved_sql (name, grp, description, sql, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![name, group, description.trim(), sql, now],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

pub fn delete_sql(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM saved_sql WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Most recent runs across all queries and saved statements.
pub fn recent_runs(conn: &Connection, limit: i64) -> Result<Vec<RunRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, query_id, params_json, ran_at, ok, rows, elapsed_ms, error
             FROM run_history ORDER BY id DESC LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![limit], |r| {
            Ok(RunRecord {
                id: r.get(0)?,
                query_id: r.get(1)?,
                params_json: r.get(2)?,
                ran_at: r.get(3)?,
                ok: r.get::<_, i64>(4)? != 0,
                rows: r.get(5)?,
                elapsed_ms: r.get(6)?,
                error: r.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// One catalog query by id, from the metadata table. (The search index does not hold the id,
/// so searching for it finds nothing.)
pub fn find(conn: &Connection, query_id: &str) -> Result<Option<QueryHit>, String> {
    conn.query_row(
        "SELECT query_id, path, description, tags, favorite, last_run_at, run_count FROM query_meta WHERE query_id = ?1",
        params![query_id],
        |r| {
            Ok(QueryHit {
                query_id: r.get(0)?,
                path: r.get(1)?,
                description: r.get(2)?,
                tags: r.get(3)?,
                favorite: r.get::<_, i64>(4)? != 0,
                last_run_at: r.get(5)?,
                run_count: r.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// A PostgreSQL the app can query. The password is not here: it is in the system keychain.
/// With `via_ssh` the query runs inside `container` on `ssh_host` (see remote.rs); otherwise it connects
/// to `host:port` directly.
#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct Datasource {
    pub id: i64,
    pub name: String,
    pub host: String,
    pub port: i64,
    pub database: String,
    pub user: String,
    pub via_ssh: bool,
    pub ssh_host: String,
    pub container: String,
    pub password_saved: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatasourceInput {
    pub name: String,
    pub host: String,
    pub port: i64,
    pub database: String,
    pub user: String,
    pub via_ssh: bool,
    pub ssh_host: String,
    pub container: String,
}

const DATASOURCE_COLUMNS: &str =
    "id, name, host, port, database, user, via_ssh, ssh_host, container, password_saved";

fn datasource_row(r: &rusqlite::Row) -> rusqlite::Result<Datasource> {
    Ok(Datasource {
        id: r.get(0)?,
        name: r.get(1)?,
        host: r.get(2)?,
        port: r.get(3)?,
        database: r.get(4)?,
        user: r.get(5)?,
        via_ssh: r.get::<_, i64>(6)? != 0,
        ssh_host: r.get(7)?,
        container: r.get(8)?,
        password_saved: r.get::<_, i64>(9)? != 0,
    })
}

pub fn list_datasources(conn: &Connection) -> Result<Vec<Datasource>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {DATASOURCE_COLUMNS} FROM datasource ORDER BY name COLLATE NOCASE"))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], datasource_row).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn get_datasource(conn: &Connection, id: i64) -> Result<Option<Datasource>, String> {
    conn.query_row(
        &format!("SELECT {DATASOURCE_COLUMNS} FROM datasource WHERE id = ?1"),
        params![id],
        datasource_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Creates (id None) or updates a data source. Returns its id. `password_saved` says whether the keychain holds one.
pub fn save_datasource(
    conn: &Connection,
    id: Option<i64>,
    input: &DatasourceInput,
    password_saved: bool,
) -> Result<i64, String> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err("the data source needs a name".into());
    }
    if !(1..=65535).contains(&input.port) {
        return Err("the port must be between 1 and 65535".into());
    }
    let taken: Option<i64> = conn
        .query_row("SELECT id FROM datasource WHERE name = ?1", params![name], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if taken.is_some() && taken != id {
        return Err(format!("a data source named {name} already exists"));
    }
    match id {
        Some(id) => {
            conn.execute(
                "UPDATE datasource SET name = ?2, host = ?3, port = ?4, database = ?5, user = ?6,
                 via_ssh = ?7, ssh_host = ?8, container = ?9, password_saved = ?10, updated_at = ?11 WHERE id = ?1",
                params![
                    id,
                    name,
                    input.host.trim(),
                    input.port,
                    input.database.trim(),
                    input.user.trim(),
                    input.via_ssh as i64,
                    input.ssh_host.trim(),
                    input.container.trim(),
                    password_saved as i64,
                    now_ms()
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO datasource (name, host, port, database, user, via_ssh, ssh_host, container, password_saved, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    name,
                    input.host.trim(),
                    input.port,
                    input.database.trim(),
                    input.user.trim(),
                    input.via_ssh as i64,
                    input.ssh_host.trim(),
                    input.container.trim(),
                    password_saved as i64,
                    now_ms()
                ],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

/// Deletes a data source. Statements and environments that used it fall back to the default.
pub fn delete_datasource(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("UPDATE saved_sql SET datasource_id = NULL WHERE datasource_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.execute("UPDATE environment SET datasource_id = NULL WHERE datasource_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.execute("UPDATE query_meta SET datasource_id = NULL WHERE datasource_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM datasource WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// The data source a catalog query runs on, if one was chosen.
pub fn query_datasource(conn: &Connection, query_id: &str) -> Result<Option<i64>, String> {
    conn.query_row(
        "SELECT datasource_id FROM query_meta WHERE query_id = ?1",
        params![query_id],
        |r| r.get::<_, Option<i64>>(0),
    )
    .optional()
    .map(|v| v.flatten())
    .map_err(|e| e.to_string())
}

pub fn set_query_datasource(conn: &Connection, query_id: &str, datasource_id: Option<i64>) -> Result<(), String> {
    conn.execute(
        "UPDATE query_meta SET datasource_id = ?2 WHERE query_id = ?1",
        params![query_id, datasource_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn set_saved_datasource(conn: &Connection, id: i64, datasource_id: Option<i64>) -> Result<(), String> {
    conn.execute(
        "UPDATE saved_sql SET datasource_id = ?2 WHERE id = ?1",
        params![id, datasource_id],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// A saved shell command, run on a server over ssh (see server_commands.rs). Runs are recorded under query_id "command:<id>".
#[derive(Debug, Serialize)]
pub struct SavedCommand {
    pub id: i64,
    pub name: String,
    /// Folder the command is filed under; empty means the top of the tree.
    pub group: String,
    pub description: String,
    pub command: String,
    /// The ssh alias the command runs on by default.
    pub host: String,
    pub updated_at: i64,
}

pub fn list_commands(conn: &Connection) -> Result<Vec<SavedCommand>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, grp, description, command, host, updated_at FROM saved_command ORDER BY grp, updated_at DESC, id DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(SavedCommand {
                id: r.get(0)?,
                name: r.get(1)?,
                group: r.get(2)?,
                description: r.get(3)?,
                command: r.get(4)?,
                host: r.get(5)?,
                updated_at: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn save_command(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    group: &str,
    description: &str,
    command: &str,
    host: &str,
) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the command needs a name".into());
    }
    if command.trim().is_empty() {
        return Err("the command is empty".into());
    }
    let group = group.trim();
    if !group.is_empty() {
        create_group(conn, group)?;
    }
    match id {
        Some(id) => {
            conn.execute(
                "UPDATE saved_command SET name = ?2, grp = ?3, description = ?4, command = ?5, host = ?6, updated_at = ?7 WHERE id = ?1",
                params![id, name, group, description.trim(), command, host.trim(), now_ms()],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            let now = now_ms();
            conn.execute(
                "INSERT INTO saved_command (name, grp, description, command, host, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![name, group, description.trim(), command, host.trim(), now],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

pub fn delete_command(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM saved_command WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Command folders, with how many commands each holds. A folder exists on its own, even when empty.
pub fn list_command_groups(conn: &Connection) -> Result<Vec<SavedGroup>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT g.name, (SELECT COUNT(*) FROM saved_command c WHERE c.grp = g.name) AS n
             FROM command_group g
             UNION
             SELECT grp, COUNT(*) FROM saved_command WHERE grp <> '' GROUP BY grp
             ORDER BY 1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| Ok(SavedGroup { name: r.get(0)?, count: r.get(1)? }))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn create_command_group(conn: &Connection, name: &str) -> Result<(), String> {
    create_group(conn, name)
}

/// Deletes a command folder. Its commands are kept and moved to the top.
pub fn delete_command_group(conn: &Connection, name: &str) -> Result<(), String> {
    conn.execute("UPDATE saved_command SET grp = '' WHERE grp = ?1", params![name])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM command_group WHERE name = ?1", params![name])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A local bash script kept in the app. Its `:name` parameters are filled like a server command's:
/// from the active environment first, then from the form.
#[derive(Debug, Clone, Serialize)]
pub struct LocalScript {
    pub id: i64,
    pub name: String,
    /// Folder the script is filed under; empty means the top of the tree.
    pub group: String,
    pub description: String,
    pub body: String,
    pub updated_at: i64,
}

pub fn list_scripts(conn: &Connection) -> Result<Vec<LocalScript>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, grp, description, body, updated_at FROM local_script ORDER BY grp, name COLLATE NOCASE, id")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(LocalScript {
                id: r.get(0)?,
                name: r.get(1)?,
                group: r.get(2)?,
                description: r.get(3)?,
                body: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn save_script(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    group: &str,
    description: &str,
    body: &str,
) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the script needs a name".into());
    }
    if body.trim().is_empty() {
        return Err("the script is empty".into());
    }
    let group = group.trim();
    match id {
        Some(id) => {
            conn.execute(
                "UPDATE local_script SET name = ?2, grp = ?3, description = ?4, body = ?5, updated_at = ?6 WHERE id = ?1",
                params![id, name, group, description.trim(), body, now_ms()],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            let now = now_ms();
            conn.execute(
                "INSERT INTO local_script (name, grp, description, body, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
                params![name, group, description.trim(), body, now],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

pub fn delete_script(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM local_script WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// An AI analysis kept: what was sent (as edited) and the answer.
#[derive(Debug, Serialize)]
pub struct SavedAnalysis {
    pub id: i64,
    pub name: String,
    pub source: String,
    pub request: String,
    pub answer: String,
    pub model: String,
    pub created_at: i64,
    pub updated_at: i64,
    /// The saved result the analysis was made from.
    pub result_id: Option<i64>,
}

fn analysis_row(r: &rusqlite::Row) -> rusqlite::Result<SavedAnalysis> {
    Ok(SavedAnalysis {
        id: r.get(0)?,
        name: r.get(1)?,
        source: r.get(2)?,
        request: r.get(3)?,
        answer: r.get(4)?,
        model: r.get(5)?,
        created_at: r.get(6)?,
        updated_at: r.get(7)?,
        result_id: r.get(8)?,
    })
}

const ANALYSIS_COLUMNS: &str = "id, name, source, request, answer, model, created_at, updated_at, result_id";

pub fn list_analyses(conn: &Connection) -> Result<Vec<SavedAnalysis>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {ANALYSIS_COLUMNS} FROM saved_analysis ORDER BY updated_at DESC, id DESC"))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], analysis_row).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn get_analysis(conn: &Connection, id: i64) -> Result<Option<SavedAnalysis>, String> {
    conn.query_row(
        &format!("SELECT {ANALYSIS_COLUMNS} FROM saved_analysis WHERE id = ?1"),
        params![id],
        analysis_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Creates (id None) or updates an analysis. Returns its id. `result_id` is the saved result it was made from.
#[allow(clippy::too_many_arguments)]
pub fn save_analysis(
    conn: &Connection,
    id: Option<i64>,
    name: &str,
    source: &str,
    request: &str,
    answer: &str,
    model: &str,
    result_id: Option<i64>,
) -> Result<i64, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("the analysis needs a name".into());
    }
    let now = now_ms();
    match id {
        Some(id) => {
            conn.execute(
                "UPDATE saved_analysis SET name = ?2, source = ?3, request = ?4, answer = ?5, model = ?6, updated_at = ?7, result_id = ?8 WHERE id = ?1",
                params![id, name, source, request, answer, model, now, result_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(id)
        }
        None => {
            conn.execute(
                "INSERT INTO saved_analysis (name, source, request, answer, model, created_at, updated_at, result_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
                params![name, source, request, answer, model, now, result_id],
            )
            .map_err(|e| e.to_string())?;
            Ok(conn.last_insert_rowid())
        }
    }
}

pub fn delete_analysis(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM saved_analysis WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// A result kept on disk: the CSV file in the app's results folder, and the facts about it here.
#[derive(Debug, Serialize)]
pub struct SavedResult {
    pub id: i64,
    pub name: String,
    /// The parameter values the result was made with (JSON), if it had any.
    pub params: Option<String>,
    /// Where it came from: a catalog query id, or "saved:<id>".
    pub source: String,
    pub datasource_id: Option<i64>,
    pub sql: String,
    pub row_count: i64,
    pub truncated: bool,
    pub created_at: i64,
    /// File name inside the results folder.
    pub file: String,
}

fn saved_result_row(r: &rusqlite::Row) -> rusqlite::Result<SavedResult> {
    Ok(SavedResult {
        id: r.get(0)?,
        name: r.get(1)?,
        source: r.get(2)?,
        datasource_id: r.get(3)?,
        sql: r.get(4)?,
        row_count: r.get(5)?,
        truncated: r.get::<_, i64>(6)? != 0,
        created_at: r.get(7)?,
        file: r.get(8)?,
        params: r.get(9)?,
    })
}

const SAVED_RESULT_COLUMNS: &str =
    "id, name, source, datasource_id, sql, row_count, truncated, created_at, file, params";

pub fn list_saved_results(conn: &Connection) -> Result<Vec<SavedResult>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT {SAVED_RESULT_COLUMNS} FROM saved_result ORDER BY id DESC"))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], saved_result_row).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn get_saved_result(conn: &Connection, id: i64) -> Result<Option<SavedResult>, String> {
    conn.query_row(
        &format!("SELECT {SAVED_RESULT_COLUMNS} FROM saved_result WHERE id = ?1"),
        params![id],
        saved_result_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

#[allow(clippy::too_many_arguments)]
pub fn insert_saved_result(
    conn: &Connection,
    name: &str,
    source: &str,
    datasource_id: Option<i64>,
    sql: &str,
    row_count: i64,
    truncated: bool,
    file: &str,
    params_json: Option<&str>,
) -> Result<i64, String> {
    conn.execute(
        "INSERT INTO saved_result (name, source, datasource_id, sql, row_count, truncated, created_at, file, params)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![name.trim(), source, datasource_id, sql, row_count, truncated as i64, now_ms(), file, params_json],
    )
    .map_err(|e| e.to_string())?;
    Ok(conn.last_insert_rowid())
}

/// Deletes a kept result. The analyses made from it go with it.
pub fn delete_saved_result(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute("DELETE FROM saved_analysis WHERE result_id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM saved_result WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_catalog(files: &[(&str, &str)]) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("sqlpool-store-{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        for (name, body) in files {
            fs::write(dir.join(name), body).unwrap();
        }
        dir
    }

    fn query(id: &str, description: &str, body: &str) -> String {
        format!("/*---\nid: {id}\ndescription: {description}\nparams: {{}}\n---*/\n{body}\n")
    }

    fn ids(hits: &[QueryHit]) -> Vec<String> {
        hits.iter().map(|h| h.query_id.clone()).collect()
    }

    #[test]
    fn sync_search_favorites_tags_and_history() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let dir = temp_catalog(&[
            ("a.sql", &query("qb.sponsor.trend", "Sponsor trend by day", "SELECT views FROM qb_analytics_hour")),
            ("b.sql", &query("qb.ops.backlog", "Open backlog", "SELECT id FROM orders WHERE status = 'open'")),
            ("broken.sql", "no front matter here"),
        ]);

        let report = sync_catalog(&mut conn, &dir).unwrap();
        assert_eq!(report.indexed, 2);
        assert_eq!(report.skipped.len(), 1);

        // Empty query lists everything; FTS matches body text of 3+ characters.
        assert_eq!(search(&conn, "").unwrap().len(), 2);
        assert_eq!(ids(&search(&conn, "qb_analytics_hour").unwrap()), vec!["qb.sponsor.trend"]);
        // Short term falls back to LIKE on description.
        assert_eq!(ids(&search(&conn, "op").unwrap()), vec!["qb.ops.backlog"]);
        // Several terms must all match.
        assert!(search(&conn, "sponsor orders").unwrap().is_empty());

        // Favorites float to the top; tags are searchable.
        set_meta(&conn, "qb.ops.backlog", Some(true), Some("weekly, ops")).unwrap();
        let all = search(&conn, "").unwrap();
        assert_eq!(all[0].query_id, "qb.ops.backlog");
        assert!(all[0].favorite);
        assert_eq!(ids(&search(&conn, "weekly").unwrap()), vec!["qb.ops.backlog"]);

        // Run history: counts, last run, newest first.
        let run = |ok: bool| RunInput {
            sql: "",
            query_id: "qb.sponsor.trend",
            path: "a.sql",
            description: "Sponsor trend by day",
            params_json: "{\"bucket\":\"DAY\"}",
            ok,
            rows: if ok { Some(3) } else { None },
            elapsed_ms: Some(12),
            error: if ok { None } else { Some("boom") },
        };
        record_run(&conn, &run(true)).unwrap();
        record_run(&conn, &run(false)).unwrap();
        let hits = search(&conn, "sponsor").unwrap();
        assert_eq!(hits[0].run_count, 2);
        assert!(hits[0].last_run_at.is_some());
        let h = history(&conn, "qb.sponsor.trend", 10).unwrap();
        assert_eq!(h.len(), 2);
        assert!(!h[0].ok);
        assert_eq!(h[0].error.as_deref(), Some("boom"));
        assert!(h[1].ok);

        // Re-sync keeps favorites, tags and history.
        sync_catalog(&mut conn, &dir).unwrap();
        let kept = find(&conn, "qb.ops.backlog").unwrap().unwrap();
        assert!(kept.favorite);
        assert_eq!(kept.tags, "weekly, ops");
        assert_eq!(history(&conn, "qb.sponsor.trend", 10).unwrap().len(), 2);

        // A deleted file disappears from search but its history stays.
        fs::remove_file(dir.join("a.sql")).unwrap();
        sync_catalog(&mut conn, &dir).unwrap();
        assert_eq!(ids(&search(&conn, "").unwrap()), vec!["qb.ops.backlog"]);
        assert_eq!(history(&conn, "qb.sponsor.trend", 10).unwrap().len(), 2);

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn groups_exist_on_their_own_and_keep_statements_on_delete() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        create_group(&conn, "  Orders ").unwrap();
        create_group(&conn, "Orders").unwrap(); // no duplicate
        assert!(create_group(&conn, "   ").is_err());
        let names: Vec<_> = list_groups(&conn).unwrap().into_iter().map(|g| (g.name, g.count)).collect();
        assert_eq!(names, vec![("Orders".to_string(), 0)]);

        save_sql(&conn, None, "Open", "Orders", "", "SELECT 1").unwrap();
        save_sql(&conn, None, "Loose", "Reports", "", "SELECT 2").unwrap();
        let counts: Vec<_> = list_groups(&conn).unwrap().into_iter().map(|g| (g.name, g.count)).collect();
        assert_eq!(counts, vec![("Orders".to_string(), 1), ("Reports".to_string(), 1)]);

        delete_group(&conn, "Orders").unwrap();
        let after: Vec<_> = list_groups(&conn).unwrap().into_iter().map(|g| g.name).collect();
        assert_eq!(after, vec!["Reports".to_string()]);
        let open = list_saved(&conn).unwrap().into_iter().find(|s| s.name == "Open").unwrap();
        assert_eq!(open.group, "");
    }

    #[test]
    fn environments_form_a_tree_and_drop_empty_values() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let mut v = Values::new();
        v.insert("operatorId".into(), "op-1".into());
        v.insert("asOf".into(), String::new());
        save_environment(&conn, " Base ", None, " shared ", &v, None).unwrap();
        save_environment(&conn, "Staging", Some("Base"), "", &Values::new(), None).unwrap();
        let all = list_environments(&conn).unwrap();
        let base = all.iter().find(|e| e.name == "Base").unwrap();
        assert_eq!(base.description, "shared");
        assert!(!base.values.contains_key("asOf"));
        assert_eq!(all.iter().find(|e| e.name == "Staging").unwrap().parent.as_deref(), Some("Base"));

        // a cycle and a self-parent are refused
        assert!(save_environment(&conn, "Base", Some("Staging"), "", &v, None).is_err());
        assert!(save_environment(&conn, "Base", Some("Base"), "", &v, None).is_err());
        assert!(save_environment(&conn, "X", Some("Missing"), "", &v, None).is_err());

        // rename keeps the children attached
        rename_environment(&conn, "Base", "Root").unwrap();
        assert_eq!(list_environments(&conn).unwrap().iter().find(|e| e.name == "Staging").unwrap().parent.as_deref(), Some("Root"));

        // deleting a parent moves its children to the top
        delete_environment(&conn, "Root").unwrap();
        let left = list_environments(&conn).unwrap();
        assert_eq!(left.len(), 1);
        assert!(left[0].parent.is_none());
        assert!(save_environment(&conn, "  ", None, "", &v, None).is_err());
    }

    #[test]
    fn saved_sql_crud_and_recent_runs() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let id = save_sql(&conn, None, "  Open orders ", " Orders ", "by status", "SELECT * FROM orders WHERE status = :status").unwrap();
        let all = list_saved(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "Open orders");
        assert_eq!(all[0].description, "by status");
        assert_eq!(all[0].group, "Orders");

        save_sql(&conn, Some(id), "Open orders v2", "Reports", "", "SELECT 1").unwrap();
        let all = list_saved(&conn).unwrap();
        assert_eq!(all[0].name, "Open orders v2");
        assert_eq!(all[0].sql, "SELECT 1");

        assert_eq!(all[0].group, "Reports");
        assert!(save_sql(&conn, None, "  ", "", "", "SELECT 1").is_err());
        assert!(save_sql(&conn, None, "x", "", "", "   ").is_err());

        let key = format!("saved:{id}");
        for ok in [true, false] {
            record_run(&conn, &RunInput {
                sql: "",
                query_id: &key,
                path: "",
                description: "Open orders v2",
                params_json: "{\"status\":\"open\"}",
                ok,
                rows: None,
                elapsed_ms: None,
                error: None,
            })
            .unwrap();
        }
        let runs = recent_runs(&conn, 10).unwrap();
        assert_eq!(runs.len(), 2);
        assert!(!runs[0].ok); // newest first
        assert_eq!(history(&conn, &key, 10).unwrap().len(), 2);

        delete_sql(&conn, id).unwrap();
        assert!(list_saved(&conn).unwrap().is_empty());
        // Runs of a deleted statement stay in history.
        assert_eq!(recent_runs(&conn, 10).unwrap().len(), 2);
    }

    #[test]
    fn data_sources_results_and_their_links() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let input = |name: &str, via_ssh: bool| DatasourceInput {
            name: name.into(),
            host: "db.example".into(),
            port: 5432,
            database: "shop".into(),
            user: "app".into(),
            via_ssh,
            ssh_host: "quizbuks-staging".into(),
            container: "postgres".into(),
        };
        let a = save_datasource(&conn, None, &input("Staging", true), true).unwrap();
        let b = save_datasource(&conn, None, &input("Prod", false), false).unwrap();
        assert!(save_datasource(&conn, None, &input("Prod", false), false).is_err(), "names are unique");
        let got = get_datasource(&conn, a).unwrap().unwrap();
        assert!(got.via_ssh && got.password_saved && got.container == "postgres");
        assert_eq!(list_datasources(&conn).unwrap().len(), 2);

        // a catalog query and a saved statement choose a data source; an environment too
        ensure_meta(&conn, "qb.app.feed", "/x.sql", "").unwrap();
        set_query_datasource(&conn, "qb.app.feed", Some(b)).unwrap();
        assert_eq!(query_datasource(&conn, "qb.app.feed").unwrap(), Some(b));
        let sid = save_sql(&conn, None, "Orders", "", "", "SELECT 1").unwrap();
        set_saved_datasource(&conn, sid, Some(a)).unwrap();
        assert_eq!(list_saved(&conn).unwrap()[0].datasource_id, Some(a));
        save_environment(&conn, "Staging env", None, "", &Values::new(), Some(a)).unwrap();
        assert_eq!(list_environments(&conn).unwrap()[0].datasource_id, Some(a));

        // deleting a data source clears the choices that pointed at it
        delete_datasource(&conn, a).unwrap();
        assert_eq!(list_saved(&conn).unwrap()[0].datasource_id, None);
        assert_eq!(list_environments(&conn).unwrap()[0].datasource_id, None);
        assert_eq!(list_datasources(&conn).unwrap().len(), 1);

        // a kept result is a row and a file name
        let rid = insert_saved_result(&conn, "feed", "qb.app.feed", Some(b), "SELECT 1", 3, false, "result-1.csv", None).unwrap();
        let r = get_saved_result(&conn, rid).unwrap().unwrap();
        assert_eq!((r.row_count, r.file.as_str()), (3, "result-1.csv"));
        assert_eq!(list_saved_results(&conn).unwrap().len(), 1);
        delete_saved_result(&conn, rid).unwrap();
        assert!(get_saved_result(&conn, rid).unwrap().is_none());

        // an analysis is kept with the result it came from, and goes when that result is deleted
        let rid2 = insert_saved_result(&conn, "r", "q", None, "SELECT 1", 1, false, "r.csv", None).unwrap();
        let aid = save_analysis(&conn, None, "a", "q", "req", "ans", "m", Some(rid2)).unwrap();
        assert_eq!(get_analysis(&conn, aid).unwrap().unwrap().result_id, Some(rid2));
        delete_saved_result(&conn, rid2).unwrap();
        assert!(get_analysis(&conn, aid).unwrap().is_none());
    }
}
