//! Generating a SQL statement or a server command from a description. The AI answers with one code block;
//! the block is checked here before it reaches the editor. Nothing is saved or run by this module.

use crate::{ai, catalog, knowledge};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// The documents' budget for a generation: the description is short, so less than for an analysis.
pub const DOC_BUDGET: i64 = 40_000;
/// Most tables and columns that are listed for the AI.
const MAX_TABLES: usize = 40;
const MAX_COLUMNS: usize = 80;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    /// "sql" or "command".
    pub kind: String,
    pub description: String,
    /// The user's extra words for the AI, optional.
    pub notes: String,
    /// What the editor holds now, so a change can be asked for. Optional.
    pub current: String,
    /// For a command: the ssh alias and the container names the user works with.
    pub server: String,
    pub doc_paths: Vec<String>,
    /// Tables (entity or physical names) whose columns the AI asked for in an earlier round.
    #[serde(default)]
    pub extra_tables: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct Prepared {
    pub system: String,
    pub user: String,
    pub docs: Vec<ai::DocRef>,
    pub tables: usize,
    pub bytes: usize,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub kind: String,
    pub text: String,
    /// The `:name` parameters of the proposal, for the parameter form.
    pub params: Vec<String>,
    /// Things the user should look at before accepting: never a refusal.
    pub warnings: Vec<String>,
    /// Tables whose columns the AI asked for. When set, the app sends them again with the columns.
    pub need_columns: Vec<String>,
}

const SYSTEM_SQL: &str = "You write one PostgreSQL statement for the quizbuks database. \
Use only the tables and columns listed in the request. The statement must be read-only: a single SELECT or WITH \
query, with no semicolon inside it and no INSERT, UPDATE, DELETE, DDL, COPY or INTO. \
Put a :name placeholder where the user should supply a value, with a short camelCase name. \
Answer with exactly one ```sql code block and nothing else. If the request cannot be met with the tables listed, \
answer with one ```sql block that holds only a line starting with `-- cannot:` and the reason. \
If a table in the list is what the request needs but its columns are not given, answer with one ```sql block that \
holds only a line `-- need-columns: NAME, NAME` naming those tables (entity or table names from the list).";

const SYSTEM_COMMAND: &str = "You write one shell command for a Linux server, to run over ssh. The server runs Docker \
containers; the request names the server and the containers. Use a :name placeholder where the user should supply a \
value, with a short camelCase name. Never write destructive commands (rm -rf, dd, mkfs, shutdown, reboot, sudo, \
recursive chmod or chown on system paths, killing with -9, deleting Docker volumes or databases). \
Answer with exactly one ```bash code block and nothing else.";

const SYSTEM_SCRIPT: &str = "You write one bash script for macOS, run on the user's Mac by the sqlpool app. Use a :name \
placeholder where the user should supply a value, with a short camelCase name; the value is quoted when the script runs. \
Never write destructive commands (rm -rf, dd, mkfs, shutdown, reboot, sudo, recursive chmod or chown on system paths, \
killing with -9). Answer with exactly one ```bash code block and nothing else.";

/// The first code block of an answer: its language and its content. A longer fence is allowed, so the content
/// can hold backticks.
pub fn extract_block(answer: &str) -> Option<(String, String)> {
    let mut lines = answer.lines();
    let mut open: Option<(usize, String)> = None;
    let mut body = Vec::new();
    for line in lines.by_ref() {
        let t = line.trim_start();
        let ticks = t.chars().take_while(|c| *c == '`').count();
        if open.is_none() {
            if ticks >= 3 {
                open = Some((ticks, t[ticks..].trim().to_string()));
            }
        } else if ticks >= open.as_ref().map(|o| o.0).unwrap_or(3) && t[ticks..].trim().is_empty() {
            let (_, lang) = open.take().unwrap_or((3, String::new()));
            return Some((lang, body.join("\n")));
        } else {
            body.push(line.to_string());
        }
    }
    None
}

/// Words that would change data or schema, which a statement for this app never may use.
const WRITE_WORDS: [&str; 16] = [
    "insert", "update", "delete", "drop", "alter", "create", "truncate", "grant", "revoke", "copy", "call", "do",
    "vacuum", "comment", "into", "reindex",
];

/// Checks a SQL statement: one SELECT or WITH, no write words. The error is the reason it was refused.
pub fn check_sql(sql: &str) -> Result<(), String> {
    let s = sql.trim().trim_end_matches(';').trim();
    if s.is_empty() {
        return Err("the answer had no statement".into());
    }
    if s.contains(';') {
        return Err("more than one statement; only one is allowed".into());
    }
    let first = s.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    if first != "select" && first != "with" {
        return Err("the statement must start with SELECT or WITH".into());
    }
    let lower = s.to_ascii_lowercase();
    for word in lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if WRITE_WORDS.contains(&word) {
            return Err(format!("the statement uses `{word}`, which is not read-only"));
        }
    }
    Ok(())
}

/// Things in a command the user should look at. Not a refusal: the user decides.
pub fn command_warnings(cmd: &str) -> Vec<String> {
    const RISKY: [(&str, &str); 14] = [
        ("rm ", "deletes files"),
        ("dd ", "writes raw devices"),
        ("mkfs", "formats a disk"),
        ("shutdown", "stops the server"),
        ("reboot", "restarts the server"),
        ("sudo ", "runs with more rights"),
        ("chmod -r", "changes permissions recursively"),
        ("chown -r", "changes owners recursively"),
        ("kill -9", "kills a process by force"),
        ("drop database", "drops a database"),
        ("truncate ", "empties a file or table"),
        ("docker rm", "removes containers"),
        ("docker system prune", "removes unused Docker data"),
        ("> /", "writes to a system path"),
    ];
    let lower = cmd.to_ascii_lowercase();
    let mut out: Vec<String> = RISKY
        .iter()
        .filter(|(pat, _)| lower.contains(pat))
        .map(|(pat, why)| format!("contains `{}`: {why}", pat.trim()))
        .collect();
    if cmd.trim().lines().count() > 1 {
        out.push("more than one line: check each line before running".into());
    }
    out
}

/// The tables and columns the description names: an entity or a table whose name appears in it.
pub fn relevant_tables(mapping: &catalog::Mapping, description: &str) -> Vec<(String, String, Vec<String>)> {
    let text = description.to_ascii_lowercase();
    let mut out: Vec<(String, String, Vec<String>)> = Vec::new();
    let mut entities: Vec<(String, String)> = mapping
        .tables
        .iter()
        .filter(|(k, _)| !k.contains('.'))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    for (k, t) in &mapping.temporal {
        entities.push((k.clone(), t.table.clone()));
    }
    for (entity, table) in entities {
        let named = text.contains(&entity.to_ascii_lowercase()) || text.contains(&table.to_ascii_lowercase());
        if !named {
            continue;
        }
        let mut cols: Vec<String> = mapping
            .tables
            .iter()
            .filter_map(|(k, v)| k.strip_prefix(&format!("{entity}.")).map(|f| format!("{f} ({v})")))
            .collect();
        if let Some(t) = mapping.temporal.get(&entity) {
            cols.extend(t.columns.iter().map(|(f, c)| format!("{f} ({c})")));
        }
        cols.truncate(MAX_COLUMNS);
        out.push((entity, table, cols));
        if out.len() >= MAX_TABLES {
            break;
        }
    }
    out
}

/// One line per entity: `Entity → table`, sorted. The table names only; no columns.
pub fn table_index(mapping: &catalog::Mapping) -> String {
    let mut lines: Vec<String> = mapping
        .tables
        .iter()
        .filter(|(k, _)| !k.contains('.'))
        .map(|(k, v)| format!("- {k} → `{v}`"))
        .collect();
    for (k, t) in &mapping.temporal {
        lines.push(format!("- {k} → `{}` (time-versioned)", t.table));
    }
    lines.sort();
    lines.join("\n") + "\n"
}

/// The request for the AI, shown and editable before it is sent.
pub fn prepare(conn: &Connection, mapping: &catalog::Mapping, input: &Input) -> Result<Prepared, String> {
    let kind = input.kind.as_str();
    if kind != "sql" && kind != "command" && kind != "script" {
        return Err("unknown kind".into());
    }
    if input.description.trim().is_empty() {
        return Err("the description is empty: write what the statement should do".into());
    }
    let mut md = String::new();
    md.push_str(&format!("# What it should do\n\n{}\n\n", input.description.trim()));
    if !input.notes.trim().is_empty() {
        md.push_str(&format!("# Notes from the user\n\n{}\n\n", input.notes.trim()));
    }
    if !input.current.trim().is_empty() {
        md.push_str(&format!("# The current text (to change)\n\n{}\n\n", ai::fenced(kind, input.current.trim())));
    }
    let mut tables = 0usize;
    if kind == "sql" {
        // Every table of the system, by entity name, so the AI can pick the ones the request means
        // (the request is often in another language than the names).
        let index = table_index(mapping);
        tables = index.lines().count();
        md.push_str("# All tables (entity → physical table)\n\n");
        md.push_str(&index);
        md.push('\n');
        // The columns of the tables the request names.
        // Names the AI asked for are added to the text the tables are found in.
        let text = format!("{} {} {}", input.description, input.notes, input.extra_tables.join(" "));
        let rel = relevant_tables(mapping, &text);
        md.push_str("# Columns of the tables named in the request\n\n");
        if rel.is_empty() {
            md.push_str("None of the tables above is named in the request by its name. Choose from the list above; if you need a column that is not listed, say so instead of guessing.\n\n");
        }
        for (entity, table, cols) in &rel {
            md.push_str(&format!("- `{table}` (entity {entity}): {}\n", cols.join(", ")));
        }
        md.push('\n');
    } else if kind == "script" {
        md.push_str("# Where it runs\n\nThe script runs on the user's Mac, with bash, from the sqlpool app.\n\n");
    } else {
        md.push_str(&format!(
            "# Server and containers\n\nThe ssh alias is `{}`. The containers the user works with are named in the request or in the notes.\n\n",
            input.server.trim()
        ));
    }
    let (docs_md, docs) = {
        let paths = knowledge::select(conn, &format!("{} {}", input.description, input.notes), DOC_BUDGET)?;
        let chosen: Vec<String> = input.doc_paths.iter().filter(|p| paths.iter().any(|h| &h.path == *p)).cloned().collect();
        let mut out = String::new();
        let mut refs = Vec::new();
        for p in &chosen {
            let body = knowledge::read(conn, p)?;
            let title = knowledge::title(conn, p).unwrap_or_else(|| p.clone());
            out.push_str(&format!("\n## {title}\n\nFile: `{p}`\n\n{body}\n"));
            refs.push(ai::DocRef { path: p.clone(), title, bytes: body.len() });
        }
        (out, refs)
    };
    md.push_str("# System documents (chosen by the user)\n");
    if docs.is_empty() {
        md.push_str("\n(none chosen)\n");
    } else {
        md.push_str(&docs_md);
    }
    let system = match kind {
        "sql" => SYSTEM_SQL,
        "script" => SYSTEM_SCRIPT,
        _ => SYSTEM_COMMAND,
    }
    .to_string();
    let bytes = system.len() + md.len();
    Ok(Prepared { system, user: md, docs, tables, bytes })
}

/// Turns the AI's answer into a proposal. A SQL answer that is not read-only is refused; a command is
/// given with its warnings.
pub fn proposal(kind: &str, answer: &str) -> Result<Proposal, String> {
    let (_lang, body) = extract_block(answer).ok_or("the answer had no code block, so nothing was proposed")?;
    let text = body.trim().to_string();
    if text.is_empty() {
        return Err("the code block was empty".into());
    }
    if kind == "sql" {
        if let Some(names) = text.lines().find_map(|l| l.trim().strip_prefix("-- need-columns:")) {
            let need: Vec<String> = names.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect();
            if need.is_empty() {
                return Err("the AI asked for columns but named no table".into());
            }
            return Ok(Proposal { kind: kind.into(), text: String::new(), params: Vec::new(), warnings: Vec::new(), need_columns: need });
        }
        if let Some(reason) = text.lines().find_map(|l| l.trim().strip_prefix("-- cannot:")) {
            return Err(format!("the AI could not write it: {}", reason.trim()));
        }
        check_sql(&text)?;
        let params = catalog::used_params(&text);
        Ok(Proposal { kind: kind.into(), text, params, warnings: Vec::new(), need_columns: Vec::new() })
    } else {
        let params = crate::server_commands::used_params(&text);
        let warnings = command_warnings(&text);
        Ok(Proposal { kind: kind.into(), text, params, warnings, need_columns: Vec::new() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_the_first_block_whatever_its_fence() {
        assert_eq!(extract_block("hi\n```sql\nSELECT 1\n```\nbye"), Some(("sql".into(), "SELECT 1".into())));
        let nested = "````bash\necho ```x```\n````";
        assert_eq!(extract_block(nested).unwrap().1, "echo ```x```");
        assert_eq!(extract_block("no block here"), None);
    }

    #[test]
    fn sql_must_be_one_read_only_select() {
        assert!(check_sql("SELECT a FROM t WHERE x = :x").is_ok());
        assert!(check_sql("WITH a AS (SELECT 1) SELECT * FROM a").is_ok());
        assert!(check_sql("DELETE FROM t").is_err());
        assert!(check_sql("SELECT 1; DROP TABLE t").is_err());
        assert!(check_sql("SELECT * INTO backup FROM t").is_err());
        assert!(check_sql("").is_err());
    }

    #[test]
    fn proposals_from_answers() {
        let p = proposal("sql", "```sql\nSELECT id FROM qb_operator WHERE name = :name\n```").unwrap();
        assert_eq!(p.params, vec!["name"]);
        assert!(proposal("sql", "```sql\n-- cannot: no such table\n```").is_err());
        assert!(proposal("sql", "no block").is_err());
        let c = proposal("command", "```bash\ndf -h /var/lib/docker\n```").unwrap();
        assert!(c.warnings.is_empty());
        let risky = proposal("command", "```bash\nsudo rm -rf /tmp/x\n```").unwrap();
        assert!(risky.warnings.len() >= 2);
    }

    #[test]
    fn tables_are_chosen_by_name_in_the_description() {
        let json = r#"{"QbOperator":"qb_operator","QbOperator.name":"name","QbOperator.status":"status","QbSponsor":"qb_sponsor"}"#;
        let m = catalog::parse_mapping(json).unwrap();
        let rel = relevant_tables(&m, "list the QbOperator rows and their status");
        assert_eq!(rel.len(), 1);
        assert_eq!(rel[0].1, "qb_operator");
        assert_eq!(rel[0].2.len(), 2);
    }
}
