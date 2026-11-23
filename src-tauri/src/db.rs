//! Read-only PostgreSQL execution.

use serde::Serialize;
use tokio_postgres::{NoTls, SimpleQueryMessage};

#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub columns: Vec<String>,
    /// Text values as PostgreSQL returns them; `None` is NULL.
    pub rows: Vec<Vec<Option<String>>>,
    pub truncated: bool,
    pub elapsed_ms: u128,
    /// The script sent to the database: the statement with its parameters rendered, in the read-only transaction.
    pub executed_sql: String,
}

/// `tokio_postgres` prints only "db error" for server errors; show the server message.
fn pg_error(e: &tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => format!("{}: {}", db.code().code(), db.message()),
        None => e.to_string(),
    }
}

/// The script that runs a statement read-only: one transaction, always rolled back.
pub fn readonly_script(sql: &str, statement_timeout_ms: u32) -> String {
    // The newline before the final statement protects against a trailing `--` comment.
    format!(
        "BEGIN READ ONLY;\nSET LOCAL statement_timeout = {statement_timeout_ms};\n{}\n;\nROLLBACK;",
        sql.trim().trim_end_matches(';')
    )
}

/// Runs one SELECT inside a READ ONLY transaction that is always rolled back.
///
/// Uses the simple query protocol, so the SQL is sent as text with the
/// parameters already rendered as literals (see `catalog::render_sql`).
pub async fn run_readonly(
    conn_str: &str,
    sql: &str,
    statement_timeout_ms: u32,
    max_rows: usize,
) -> Result<QueryResult, String> {
    if crate::remote::is_remote(conn_str) {
        return crate::remote::run(conn_str, sql, statement_timeout_ms, max_rows).await;
    }
    let started = std::time::Instant::now();
    let (client, connection) = tokio_postgres::connect(conn_str, NoTls)
        .await
        .map_err(|e| format!("connect: {}", pg_error(&e)))?;
    tokio::spawn(async move {
        let _ = connection.await;
    });

    let executed_sql = readonly_script(sql, statement_timeout_ms);
    let messages = client
        .simple_query(&executed_sql)
        .await
        .map_err(|e| pg_error(&e))?;

    let mut columns = Vec::new();
    let mut rows = Vec::new();
    let mut truncated = false;
    for message in messages {
        if let SimpleQueryMessage::Row(row) = message {
            if columns.is_empty() {
                columns = row.columns().iter().map(|c| c.name().to_string()).collect();
            }
            if rows.len() >= max_rows {
                truncated = true;
                continue;
            }
            rows.push(
                (0..row.len())
                    .map(|i| row.get(i).map(str::to_string))
                    .collect(),
            );
        }
    }

    Ok(QueryResult {
        columns,
        rows,
        truncated,
        elapsed_ms: started.elapsed().as_millis(),
        executed_sql,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs a running PostgreSQL. Set SQLPOOL_TEST_PG, or it skips.
    fn conn() -> Option<String> {
        std::env::var("SQLPOOL_TEST_PG").ok()
    }

    #[tokio::test]
    async fn runs_select_and_enforces_read_only_and_limits() {
        let Some(conn) = conn() else { return };

        let r = run_readonly(&conn, "SELECT 1 AS a, 'x'::text AS b", 5_000, 10)
            .await
            .unwrap();
        assert_eq!(r.columns, vec!["a", "b"]);
        assert_eq!(r.rows, vec![vec![Some("1".into()), Some("x".into())]]);

        let r = run_readonly(&conn, "SELECT generate_series(1, 10)", 5_000, 3)
            .await
            .unwrap();
        assert_eq!(r.rows.len(), 3);
        assert!(r.truncated);

        let err = run_readonly(&conn, "CREATE TEMP TABLE t_ro (x int)", 5_000, 10)
            .await
            .unwrap_err();
        assert!(err.contains("read-only"), "got: {err}");

        let err = run_readonly(&conn, "SELECT pg_sleep(2)", 200, 10)
            .await
            .unwrap_err();
        assert!(err.contains("statement timeout"), "got: {err}");

        // A trailing line comment must not swallow the ROLLBACK.
        let r = run_readonly(&conn, "SELECT 7 AS n -- comment", 5_000, 10)
            .await
            .unwrap();
        assert_eq!(r.rows[0][0].as_deref(), Some("7"));
    }
}

#[cfg(test)]
mod e2e_tests {
    use super::*;
    use crate::catalog;
    use std::collections::BTreeMap;

    /// Runs the real analytics-trend query end to end. Creates the tables it needs in the
    /// SQLPOOL_TEST_PG database (drops them first), so point it at a throwaway database only.
    #[tokio::test]
    async fn analytics_trend_end_to_end() {
        let Some(conn) = std::env::var("SQLPOOL_TEST_PG").ok() else { return };
        let source = "/Users/zhongpingxue/quizbuks/repo_quizbuks/backend/quizbuks/src/main/resources/queries/qb/sponsor/analytics-trend.sql";
        let mapping_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tools/out/mapping.json");
        let (Ok(src), Ok(map_text)) = (std::fs::read_to_string(source), std::fs::read_to_string(mapping_path)) else {
            return;
        };

        // Fixture tables with the physical names from the mapping.
        let (client, connection) = tokio_postgres::connect(&conn, NoTls).await.unwrap();
        tokio::spawn(async move { let _ = connection.await; });
        client
            .batch_execute(
                "DROP TABLE IF EXISTS qb_analytics_hour, qb_publication_card, qb_stats_run;
                 CREATE TABLE qb_publication_card (publication_id uuid PRIMARY KEY, publisher_id uuid NOT NULL);
                 CREATE TABLE qb_analytics_hour (hour_id bigserial PRIMARY KEY, publication_id uuid NOT NULL,
                   publisher_id uuid NOT NULL, hour_start timestamptz NOT NULL, views numeric(12,0) NOT NULL,
                   starts numeric(12,0) NOT NULL, completions numeric(12,0) NOT NULL, passes numeric(12,0) NOT NULL,
                   winners numeric(12,0) NOT NULL, spent numeric(12,0) NOT NULL, gap boolean NOT NULL);
                 CREATE TABLE qb_stats_run (job text NOT NULL, day_zone text NOT NULL);
                 INSERT INTO qb_publication_card VALUES ('11111111-1111-1111-1111-111111111111', '22222222-2222-2222-2222-222222222222');
                 INSERT INTO qb_analytics_hour (publication_id, publisher_id, hour_start, views, starts, completions, passes, winners, spent, gap)
                   VALUES ('11111111-1111-1111-1111-111111111111', '22222222-2222-2222-2222-222222222222',
                           '2026-01-01T02:00:00Z', 5, 3, 2, 1, 1, 100, false),
                          ('11111111-1111-1111-1111-111111111111', '22222222-2222-2222-2222-222222222222',
                           '2026-01-01T02:00:00Z', 4, 0, 0, 0, 0, 0, true);
                 INSERT INTO qb_stats_run VALUES ('qb.analytics', 'UTC');",
            )
            .await
            .unwrap();

        let (front, body) = catalog::parse_source(&src).unwrap();
        assert_eq!(front.id, "qb.sponsor.analytics.trend");
        let mapping = catalog::parse_mapping(&map_text).unwrap();
        let resolved = catalog::resolve_placeholders(&body, &mapping, &catalog::TimeSql::default()).unwrap();

        let mut values = BTreeMap::new();
        values.insert("publicationId".to_string(), None);
        values.insert("from".to_string(), Some("2026-01-01T00:00:00Z".to_string()));
        values.insert("to".to_string(), Some("2026-01-01T04:00:00Z".to_string()));
        values.insert("bucket".to_string(), Some("HOUR".to_string()));
        let sql = catalog::render_sql(&resolved, &values).unwrap();

        // HOUR bucket over 4 hours: 4 buckets, zeros included.
        let r = run_readonly(&conn, &sql, 10_000, 100).await.unwrap();
        assert_eq!(r.rows.len(), 4, "columns: {:?}", r.columns);
        let views_col = r.columns.iter().position(|c| c == "views").unwrap();
        let total: i64 = r.rows.iter().map(|row| row[views_col].as_deref().unwrap().parse::<i64>().unwrap()).sum();
        assert_eq!(total, 9);
        // The query has no ORDER BY, so find the 02:00 bucket by its start time.
        let start_col = r.columns.iter().position(|c| c == "bucketstart").unwrap();
        let gap_col = r.columns.iter().position(|c| c == "gap").unwrap();
        let two = r
            .rows
            .iter()
            .find(|row| row[start_col].as_deref().is_some_and(|s| s.contains("02:00:00")))
            .expect("02:00 bucket present");
        assert_eq!(two[gap_col].as_deref(), Some("t"));
        let spent_col = r.columns.iter().position(|c| c == "spent").unwrap();
        assert_eq!(two[spent_col].as_deref(), Some("100"));
    }

    /// Temporal read end to end: the latest version in effect at asOf, tombstones excluded.
    /// Creates `sys_param_version` in the SQLPOOL_TEST_PG database (drops it first).
    #[tokio::test]
    async fn temporal_entity_end_to_end() {
        let Some(conn) = std::env::var("SQLPOOL_TEST_PG").ok() else { return };
        let (client, connection) = tokio_postgres::connect(&conn, NoTls).await.unwrap();
        tokio::spawn(async move { let _ = connection.await; });
        client
            .batch_execute(
                "DROP TABLE IF EXISTS sys_param_version;
                 CREATE TABLE sys_param_version (
                   param_id uuid NOT NULL, param_key text NOT NULL, param_value text,
                   version_no int NOT NULL, effect_start_time timestamptz NOT NULL,
                   created_time timestamptz NOT NULL, is_deleted boolean NOT NULL DEFAULT false,
                   PRIMARY KEY (param_id, version_no));
                 INSERT INTO sys_param_version VALUES
                   ('00000000-0000-0000-0000-000000000001', 'qb.ai.model', 'gpt-a', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', false),
                   ('00000000-0000-0000-0000-000000000001', 'qb.ai.model', 'gpt-b', 2, '2026-02-01T00:00:00Z', '2026-02-01T00:00:00Z', false),
                   ('00000000-0000-0000-0000-000000000002', 'qb.ai.effort', 'low', 1, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', false),
                   ('00000000-0000-0000-0000-000000000002', 'qb.ai.effort', 'low', 2, '2026-03-01T00:00:00Z', '2026-03-01T00:00:00Z', true);",
            )
            .await
            .unwrap();

        let source = "/*---\nid: t.sysparam\nparams:\n  asOf: { kind: { type: temporal }, required: false }\ntimeSlice: { asOf: asOf }\n---*/\nSELECT p.param_key, p.param_value FROM {{SysParam}} p ORDER BY p.param_key\n";
        let mapping_text = r#"{"SysParam": {"table": "sys_param_version", "pk": "param_id", "writeOnce": false,
            "columns": {"versionNo": "version_no", "effectStartTime": "effect_start_time",
                        "createdTime": "created_time", "deleted": "is_deleted"}}}"#;
        let (front, body) = catalog::parse_source(source).unwrap();
        let mapping = catalog::parse_mapping(mapping_text).unwrap();

        let read = |as_of: Option<&str>| {
            let mut values = BTreeMap::new();
            values.insert("asOf".to_string(), as_of.map(str::to_string));
            let time = catalog::time_sql(&front.time_slice, &values).unwrap();
            let resolved = catalog::resolve_placeholders(&body, &mapping, &time).unwrap();
            (resolved, values)
        };
        let run = |as_of: Option<&str>| {
            let (resolved, values) = read(as_of);
            let sql = catalog::render_sql(&resolved, &values).unwrap();
            let conn = conn.clone();
            async move { run_readonly(&conn, &sql, 10_000, 100).await.unwrap() }
        };

        // Before the second model version: gpt-a, and low.
        let r = run(Some("2026-01-15T00:00:00Z")).await;
        assert_eq!(
            r.rows,
            vec![
                vec![Some("qb.ai.effort".into()), Some("low".into())],
                vec![Some("qb.ai.model".into()), Some("gpt-a".into())],
            ]
        );
        // After the second version of the model; effort still low.
        let r = run(Some("2026-02-15T00:00:00Z")).await;
        assert_eq!(r.rows[1], vec![Some("qb.ai.model".into()), Some("gpt-b".into())]);
        // After the tombstone of effort: effort is gone.
        let r = run(Some("2026-03-15T00:00:00Z")).await;
        assert_eq!(r.rows, vec![vec![Some("qb.ai.model".into()), Some("gpt-b".into())]]);
        // No asOf given: now, which is after every version above.
        let r = run(None).await;
        assert_eq!(r.rows.len(), 1);
    }
}
