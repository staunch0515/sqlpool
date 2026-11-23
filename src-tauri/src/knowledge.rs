//! The system's own documents (the quizbuks repository's Markdown), indexed for full-text search.
//! A result's terms (table, column and entity names in its SQL) pick the documents that matter; the
//! selection stays within a size budget, so only a few relevant files go to the AI.

use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Larger files are skipped: they are generated or not documents.
const MAX_FILE_BYTES: u64 = 512 * 1024;

/// Folders that never hold documents worth reading.
const SKIP_DIRS: [&str; 10] = [
    "node_modules", "target", "build", "dist", ".git", ".next", "__pycache__", ".gradle", "out", "generated",
];

#[derive(Debug, Serialize, PartialEq)]
pub struct IndexReport {
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub skipped: usize,
    pub total_docs: usize,
    pub total_bytes: i64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Hit {
    pub path: String,
    pub title: String,
    pub bytes: i64,
    pub score: f64,
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') {
                collect(&path, out)?;
            }
        } else if name.to_ascii_lowercase().ends_with(".md") {
            out.push(path);
        }
    }
    Ok(())
}

/// The title: the first `# ` heading, else the file name.
fn title_of(path: &Path, body: &str) -> String {
    body.lines()
        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
        .unwrap_or_else(|| path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())
}

/// Reads every Markdown file under `dir` and makes the index match: new and changed files are read again,
/// unchanged ones are kept, and files that are gone are removed.
pub fn index(conn: &mut Connection, dir: &Path) -> Result<IndexReport, String> {
    if !dir.is_dir() {
        return Err(format!("not a folder: {}", dir.display()));
    }
    let mut files = Vec::new();
    collect(dir, &mut files).map_err(|e| format!("{}: {e}", dir.display()))?;

    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut seen: HashSet<String> = HashSet::new();
    let (mut indexed, mut unchanged, mut skipped) = (0usize, 0usize, 0usize);

    for path in &files {
        let key = path.to_string_lossy().to_string();
        seen.insert(key.clone());
        let meta = match std::fs::metadata(path) {
            Ok(m) if m.len() <= MAX_FILE_BYTES => m,
            _ => {
                skipped += 1;
                continue;
            }
        };
        let mtime = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
        let bytes = meta.len() as i64;
        let existing: Option<(i64, i64)> = tx
            .query_row("SELECT mtime, bytes FROM knowledge_doc WHERE path = ?1", params![key], |r| Ok((r.get(0)?, r.get(1)?)))
            .ok();
        if existing == Some((mtime, bytes)) {
            unchanged += 1;
            continue;
        }
        let body = match std::fs::read_to_string(path) {
            Ok(b) => b,
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let title = title_of(path, &body);
        tx.execute(
            "INSERT INTO knowledge_doc (path, title, bytes, mtime, body) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET title = excluded.title, bytes = excluded.bytes, mtime = excluded.mtime, body = excluded.body",
            params![key, title, bytes, mtime, body],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM knowledge_fts WHERE path = ?1", params![key]).map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO knowledge_fts (path, title, body) VALUES (?1, ?2, ?3)", params![key, title, body])
            .map_err(|e| e.to_string())?;
        indexed += 1;
    }

    // Documents that were in the index but are gone now (or are no longer in the folder).
    let stale: Vec<String> = {
        let mut stmt = tx.prepare("SELECT path FROM knowledge_doc").map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
    };
    let mut removed = 0usize;
    for path in stale.into_iter().filter(|p| !seen.contains(p)) {
        tx.execute("DELETE FROM knowledge_doc WHERE path = ?1", params![path]).map_err(|e| e.to_string())?;
        tx.execute("DELETE FROM knowledge_fts WHERE path = ?1", params![path]).map_err(|e| e.to_string())?;
        removed += 1;
    }
    tx.commit().map_err(|e| e.to_string())?;

    let (total_docs, total_bytes): (i64, i64) = conn
        .query_row("SELECT COUNT(*), COALESCE(SUM(bytes), 0) FROM knowledge_doc", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?;
    Ok(IndexReport {
        indexed,
        unchanged,
        removed,
        skipped,
        total_docs: total_docs as usize,
        total_bytes,
    })
}

/// Words in a SQL text or a description that name something in the system: identifiers of at least
/// four characters, minus the SQL keywords and the words every query uses.
pub fn terms_of(text: &str) -> Vec<String> {
    const COMMON: &[&str] = &[
        "select", "from", "where", "join", "left", "right", "inner", "outer", "on", "and", "not", "null",
        "group", "order", "limit", "count", "distinct", "case", "when", "then", "else", "end", "with", "union",
        "having", "exists", "between", "like", "true", "false", "text", "integer", "numeric", "timestamp",
        "varchar", "boolean", "desc", "asc", "using", "over", "coalesce",
    ];
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for raw in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if raw.len() < 4 || !raw.chars().any(|c| c.is_ascii_alphabetic()) {
            continue;
        }
        let lower = raw.to_ascii_lowercase();
        if COMMON.contains(&lower.as_str()) {
            continue;
        }
        if seen.insert(lower.clone()) {
            out.push(lower);
        }
        if out.len() >= 40 {
            break;
        }
    }
    out
}

fn is_cjk(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// The terms of a Chinese text: every three-character piece of each run of Chinese characters (the
/// trigram index finds those). Used with the identifiers of `terms_of`.
pub fn cjk_terms(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut run: Vec<char> = Vec::new();
    let flush = |run: &mut Vec<char>, out: &mut Vec<String>| {
        if run.len() >= 3 {
            for w in run.windows(3) {
                let t: String = w.iter().collect();
                if !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        run.clear();
    };
    for c in text.chars() {
        if is_cjk(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// The documents that match the terms, best first, taken while they fit in `budget_bytes`.
pub fn select(conn: &Connection, text: &str, budget_bytes: i64) -> Result<Vec<Hit>, String> {
    let mut terms = terms_of(text);
    for t in cjk_terms(text) {
        if terms.len() >= 60 {
            break;
        }
        terms.push(t);
    }
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let query = terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" OR ");
    let mut stmt = conn
        .prepare(
            "SELECT knowledge_fts.path, d.title, d.bytes, bm25(knowledge_fts)
             FROM knowledge_fts JOIN knowledge_doc d ON d.path = knowledge_fts.path
             WHERE knowledge_fts MATCH ?1
             ORDER BY bm25(knowledge_fts) LIMIT 60",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![query], |r| {
            Ok(Hit {
                path: r.get(0)?,
                title: r.get(1)?,
                bytes: r.get(2)?,
                score: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    let mut used = 0i64;
    for hit in rows {
        let hit = hit.map_err(|e| e.to_string())?;
        if used + hit.bytes <= budget_bytes {
            used += hit.bytes;
            out.push(hit);
        }
    }
    Ok(out)
}

/// The title of an indexed document, if it is indexed.
pub fn title(conn: &Connection, path: &str) -> Option<String> {
    conn.query_row("SELECT title FROM knowledge_doc WHERE path = ?1", params![path], |r| r.get(0)).ok()
}

/// The text of an indexed document.
pub fn read(conn: &Connection, path: &str) -> Result<String, String> {
    conn.query_row("SELECT body FROM knowledge_doc WHERE path = ?1", params![path], |r| r.get(0))
        .map_err(|e| format!("{path}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sqlpool-knowledge-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("docs")).unwrap();
        std::fs::create_dir_all(d.join("node_modules/x")).unwrap();
        std::fs::write(d.join("docs/operator.md"), "# Operators\n\nThe qb_operator table holds each operator and its status.\n").unwrap();
        std::fs::write(d.join("docs/sponsor.md"), "# Sponsors\n\nA sponsor links to operators through qb_sponsor_operator.\n").unwrap();
        std::fs::write(d.join("docs/unrelated.md"), "# Weather\n\nNothing about data here.\n").unwrap();
        std::fs::write(d.join("node_modules/x/skip.md"), "# Skipped\n\nqb_operator in a dependency.\n").unwrap();
        d
    }

    #[test]
    fn indexes_selects_within_budget_and_follows_changes() {
        let dir = temp_dir("a");
        let mut conn = Connection::open_in_memory().unwrap();
        crate::store::migrate(&conn).unwrap();

        let r = index(&mut conn, &dir).unwrap();
        assert_eq!(r.indexed, 3, "node_modules is skipped");
        assert_eq!(r.total_docs, 3);
        assert_eq!(index(&mut conn, &dir).unwrap().unchanged, 3, "a second run reads nothing new");

        let hits = select(&conn, "SELECT o.status FROM qb_operator o JOIN qb_sponsor_operator s", 1_000_000).unwrap();
        let names: Vec<&str> = hits.iter().map(|h| h.title.as_str()).collect();
        assert!(names.contains(&"Operators") && names.contains(&"Sponsors"), "got {names:?}");
        assert!(!names.contains(&"Weather"));

        // a tiny budget keeps only what fits
        let small = select(&conn, "SELECT * FROM qb_operator", 10).unwrap();
        assert!(small.is_empty());

        // a deleted file leaves the index
        std::fs::remove_file(dir.join("docs/sponsor.md")).unwrap();
        assert_eq!(index(&mut conn, &dir).unwrap().removed, 1);
        assert!(select(&conn, "qb_sponsor_operator", 1_000_000).unwrap().is_empty());
        assert!(read(&conn, &dir.join("docs/operator.md").to_string_lossy()).unwrap().contains("qb_operator"));
    }

    #[test]
    fn terms_skip_sql_words_and_short_words() {
        let t = terms_of("SELECT o.status, count(*) FROM qb_operator o WHERE id = 1 AND name LIKE 'x'");
        assert!(t.contains(&"qb_operator".to_string()));
        assert!(t.contains(&"status".to_string()));
        assert!(!t.contains(&"select".to_string()) && !t.contains(&"count".to_string()));
        assert!(!t.iter().any(|w| w.len() < 4));
    }
}

#[cfg(test)]
mod cjk_tests {
    use super::*;

    #[test]
    fn chinese_text_gives_trigrams_for_the_index() {
        let t = cjk_terms("请找到全部的运行商的基本信息");
        assert!(t.contains(&"运行商".to_string()));
        assert!(t.contains(&"基本信".to_string()));
        assert!(cjk_terms("ab").is_empty());
    }
}
