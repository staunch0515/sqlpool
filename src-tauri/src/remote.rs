//! Read-only queries on a PostgreSQL that runs in a Docker container on a server we reach by ssh.
//!
//! The connection is written `ssh://HOST/CONTAINER?user=USER&db=DATABASE`. The read-only script goes to
//! `psql` inside the container, through `ssh HOST docker exec -i CONTAINER psql ...`, and the result comes
//! back as CSV. Nothing from the connection string reaches the remote shell unless it passed `safe`.

use crate::db::{readonly_script, QueryResult};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Instant;

#[derive(Debug, PartialEq)]
pub struct Target {
    pub host: String,
    pub container: String,
    pub user: String,
    pub database: String,
}

pub fn is_remote(conn: &str) -> bool {
    conn.trim_start().starts_with("ssh://")
}

/// Letters, digits and `._-@:` only, and no leading `-` (it would read as an ssh option).
fn safe(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 128
        && !part.starts_with('-')
        && part.chars().all(|c| c.is_ascii_alphanumeric() || "._-@:".contains(c))
}

/// Parses `ssh://HOST/CONTAINER?user=USER&db=DATABASE`. `user` defaults to `postgres`; `db` is required.
pub fn parse(conn: &str) -> Result<Target, String> {
    let rest = conn
        .trim()
        .strip_prefix("ssh://")
        .ok_or("not an ssh:// connection")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (host, container) = path
        .split_once('/')
        .ok_or("expected ssh://HOST/CONTAINER?user=USER&db=DATABASE")?;
    let mut user = String::new();
    let mut database = String::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (key, value) = pair.split_once('=').ok_or("expected key=value in the query")?;
        match key {
            "user" => user = value.to_string(),
            "db" | "database" => database = value.to_string(),
            other => return Err(format!("unknown option {other}")),
        }
    }
    if user.is_empty() {
        user = "postgres".to_string();
    }
    if database.is_empty() {
        return Err("the database is required: add &db=NAME".into());
    }
    for (name, value) in [("host", host), ("container", container), ("user", user.as_str()), ("db", database.as_str())] {
        if !safe(value) {
            return Err(format!("{name} has characters that are not allowed: {value}"));
        }
    }
    Ok(Target {
        host: host.to_string(),
        container: container.to_string(),
        user,
        database,
    })
}

/// The command the server runs. Every part was checked by `parse`, so no quoting is needed.
pub fn remote_command(t: &Target) -> String {
    format!(
        "docker exec -i {} psql -X -q --csv -v ON_ERROR_STOP=1 -U {} -d {} -f -",
        t.container, t.user, t.database
    )
}

/// Runs the read-only script on the server and returns the result, like `db::run_readonly`.
pub async fn run(
    conn: &str,
    sql: &str,
    statement_timeout_ms: u32,
    max_rows: usize,
) -> Result<QueryResult, String> {
    let target = parse(conn)?;
    let started = Instant::now();
    let script = readonly_script(sql, statement_timeout_ms);
    let sent = script.clone();
    let host = target.host.clone();
    let remote = remote_command(&target);

    let output = tokio::task::spawn_blocking(move || -> Result<std::process::Output, String> {
        let mut child = Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host.as_str(), remote.as_str()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("ssh: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(script.as_bytes())
                .map_err(|e| format!("ssh: {e}"))?;
        }
        child.wait_with_output().map_err(|e| format!("ssh: {e}"))
    })
    .await
    .map_err(|e| e.to_string())??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("ssh exited with {}", output.status)
        } else {
            stderr
        });
    }

    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let mut records = crate::csv::parse(&text).into_iter();
    let columns: Vec<String> = records
        .next()
        .unwrap_or_default()
        .into_iter()
        .map(|v| v.unwrap_or_default())
        .collect();
    let mut rows = Vec::new();
    let mut truncated = false;
    for row in records {
        if rows.len() >= max_rows {
            truncated = true;
            break;
        }
        rows.push(row);
    }
    Ok(QueryResult {
        columns,
        rows,
        truncated,
        elapsed_ms: started.elapsed().as_millis(),
        executed_sql: sent,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_connection_and_refuses_unsafe_parts() {
        let t = parse("ssh://quizbuks-staging/postgres?user=postgres&db=quizbuks_staging").unwrap();
        assert_eq!(
            t,
            Target {
                host: "quizbuks-staging".into(),
                container: "postgres".into(),
                user: "postgres".into(),
                database: "quizbuks_staging".into(),
            }
        );
        assert_eq!(parse("ssh://h/c?db=d").unwrap().user, "postgres");
        assert!(parse("ssh://h/c").is_err(), "database is required");
        assert!(parse("ssh://h/c?db=d; rm -rf /").is_err());
        assert!(parse("ssh://-oProxyCommand=x/c?db=d").is_err(), "a host must not read as an option");
        assert!(parse("ssh://h/c?db=d&x=1").is_err());
        assert!(parse("postgres://h/db").is_err());
        assert!(is_remote(" ssh://h/c?db=d"));
        assert!(!is_remote("postgres://u:p@h/db"));
    }

    #[test]
    fn builds_the_remote_command_without_a_tty() {
        let t = parse("ssh://h/postgres?user=postgres&db=quizbuks_staging").unwrap();
        let cmd = remote_command(&t);
        assert_eq!(
            cmd,
            "docker exec -i postgres psql -X -q --csv -v ON_ERROR_STOP=1 -U postgres -d quizbuks_staging -f -"
        );
        assert!(!cmd.contains(" -it"));
    }
}
#[cfg(test)]
mod live_tests {
    use super::*;

    /// Needs the server: set SQLPOOL_TEST_SSH to an ssh:// connection (e.g. the staging one). Skips otherwise.
    #[tokio::test]
    async fn runs_a_select_on_the_server() {
        let Some(conn) = std::env::var("SQLPOOL_TEST_SSH").ok() else { return };
        let r = run(&conn, "SELECT 1 AS one, 'a,b' AS two, NULL::text AS three", 10_000, 10)
            .await
            .unwrap();
        assert_eq!(r.columns, vec!["one", "two", "three"]);
        assert_eq!(r.rows, vec![vec![Some("1".into()), Some("a,b".into()), None]]);

        let err = run(&conn, "CREATE TEMP TABLE t_ro (x int)", 10_000, 10)
            .await
            .unwrap_err();
        assert!(err.contains("read-only"), "got: {err}");
    }
}
