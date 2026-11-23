//! AI analysis of a result. The request is built here and shown to the user first (see `prepare`); only
//! after the user confirms is it sent (see `send`). The API key stays in the keychain and is read here,
//! so it never travels to the frontend.

use crate::{knowledge, secrets};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

pub const KEY_NAME: &str = "anthropic-api-key";
const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
/// Rows of the result that go to the AI. The rest are only counted.
pub const SAMPLE_ROWS: usize = 20;
/// Longest value of a cell that is sent; longer ones are cut.
const MAX_CELL: usize = 300;

const SYSTEM: &str = "You analyze the results of queries and server commands of the quizbuks system. \
The documents given are the system's own; use them for the meaning of tables, columns, statuses and codes. \
Answer in Chinese. Structure the answer as: 1) findings from the data; 2) what the documents say about them; \
3) your own inference, marked as inference; 4) what you could not tell because the documents or the data lack it. \
Never invent a meaning for a column or status code that the documents do not give. \
The data may have been redacted: values shown as [redacted] are personal data that were removed on purpose. \
The message is Markdown; its sections are the question, what produced the result, the description, the result \
sample and the documents the user chose. Sections the user left out are not there, and that is on purpose.";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Input {
    /// What produced the result: the SQL (with parameters rendered) or the ssh command.
    pub executed: String,
    pub description: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Option<String>>>,
    pub total_rows: usize,
    pub truncated: bool,
    pub question: String,
    /// The documents to include, by path, as the user chose them.
    pub doc_paths: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct DocRef {
    pub path: String,
    pub title: String,
    pub bytes: usize,
}

/// What would be sent: shown to the user before anything leaves the machine.
#[derive(Debug, Serialize)]
pub struct Prepared {
    pub system: String,
    pub user: String,
    pub docs: Vec<DocRef>,
    pub sample_rows: usize,
    pub redacted_cells: usize,
    pub bytes: usize,
}

/// A column whose name says it holds personal data: its values are never sent.
fn sensitive_column(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["email", "mail", "phone", "mobile", "password", "passwd", "token", "secret", "address", "iban", "card", "ssn", "tax", "pwd"]
        .iter()
        .any(|w| n.contains(w))
}

/// Replaces e-mail addresses and long digit runs (phone numbers, card numbers) in a value.
pub fn redact_value(v: &str) -> (String, bool) {
    let mut out = String::new();
    let mut changed = false;
    for word in v.split(' ') {
        let is_mail = word.contains('@') && word.contains('.');
        let digits = word.chars().filter(|c| c.is_ascii_digit()).count();
        if is_mail || digits >= 8 {
            out.push_str("[redacted]");
            changed = true;
        } else {
            out.push_str(word);
        }
        out.push(' ');
    }
    (out.trim_end().to_string(), changed)
}

fn cut(v: &str) -> String {
    if v.chars().count() <= MAX_CELL {
        return v.to_string();
    }
    let head: String = v.chars().take(MAX_CELL).collect();
    format!("{head}…")
}

/// A fenced block whose fence is longer than any run of backticks inside, so the content cannot close it.
pub fn fenced(lang: &str, content: &str) -> String {
    let longest = content
        .split(|c| c != '`')
        .map(|run| run.len())
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{lang}\n{}\n{fence}\n", content.trim_end_matches('\n'))
}

/// The sample rows as text, with personal columns and personal-looking values redacted.
fn sample_text(input: &Input) -> (String, usize) {
    let mut redacted = 0usize;
    let mut out = String::new();
    out.push_str(&input.columns.join("\t"));
    out.push('\n');
    let sensitive: Vec<bool> = input.columns.iter().map(|c| sensitive_column(c)).collect();
    for row in input.rows.iter().take(SAMPLE_ROWS) {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, v)| match v {
                None => "NULL".to_string(),
                Some(_) if sensitive.get(i).copied().unwrap_or(false) => {
                    redacted += 1;
                    "[redacted]".to_string()
                }
                Some(s) => {
                    let (r, changed) = redact_value(s);
                    if changed {
                        redacted += 1;
                    }
                    cut(&r)
                }
            })
            .collect();
        out.push_str(&cells.join("\t"));
        out.push('\n');
    }
    (out, redacted)
}

/// The chosen documents, each as a Markdown section under its path and title.
fn documents_md(conn: &Connection, paths: &[String]) -> Result<(String, Vec<DocRef>), String> {
    let mut md = String::new();
    let mut refs = Vec::new();
    for p in paths {
        let body = knowledge::read(conn, p)?;
        let title = knowledge::title(conn, p).unwrap_or_else(|| p.clone());
        md.push_str(&format!("\n## {title}\n\nFile: `{p}`\n\n{body}\n"));
        refs.push(DocRef { path: p.clone(), title, bytes: body.len() });
    }
    Ok((md, refs))
}

/// Builds the request as Markdown. The user reads it, edits it, and only then sends it: what is sent is
/// the text as it was left, so anything the user removes (a document, a row, a value) stays home.
pub fn prepare(conn: &Connection, input: &Input) -> Result<Prepared, String> {
    if input.question.trim().is_empty() {
        return Err("write the question first".into());
    }
    let (samples, redacted_cells) = sample_text(input);
    let (docs_md, docs) = documents_md(conn, &input.doc_paths)?;
    let shown = input.rows.len().min(SAMPLE_ROWS);
    let mut md = String::new();
    md.push_str(&format!("# Question\n\n{}\n\n", input.question.trim()));
    md.push_str(&format!("# What produced the result\n\n{}", fenced("sql", input.executed.trim())));
    if !input.description.trim().is_empty() {
        md.push_str(&format!("\n# Description in the system\n\n{}\n", input.description.trim()));
    }
    md.push_str(&format!(
        "\n# Result\n\n{} rows{}. The first {} are below, tab-separated.\n\n{}",
        input.total_rows,
        if input.truncated { " (the app had already cut the result)" } else { "" },
        shown,
        fenced("tsv", &samples)
    ));
    md.push_str("\n# System documents (chosen by the user)\n");
    if docs.is_empty() {
        md.push_str("\n(none chosen)\n");
    } else {
        md.push_str(&docs_md);
    }
    let bytes = SYSTEM.len() + md.len();
    Ok(Prepared {
        system: SYSTEM.to_string(),
        user: md,
        docs,
        sample_rows: shown,
        redacted_cells,
        bytes,
    })
}

/// The model names the app offers.
pub const MODELS: [&str; 3] = ["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5"];

/// Sends the prepared request and returns the answer's text.
pub async fn send(system: &str, user: &str, model: &str) -> Result<String, String> {
    if !MODELS.contains(&model) {
        return Err(format!("unknown model: {model}"));
    }
    let key = secrets::get_named(KEY_NAME)?.ok_or("no API key: add it in Settings → AI")?;
    let body = serde_json::json!({
        "model": model,
        "max_tokens": 2000,
        "system": system,
        "messages": [{ "role": "user", "content": user }],
    });
    let client = reqwest::Client::new();
    let resp = client
        .post(API_URL)
        .header("x-api-key", key)
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = resp.status();
    let value: serde_json::Value = resp.json().await.map_err(|e| format!("bad answer: {e}"))?;
    if !status.is_success() {
        let msg = value["error"]["message"].as_str().unwrap_or("the service refused the request");
        return Err(format!("{status}: {msg}"));
    }
    let text: String = value["content"]
        .as_array()
        .map(|parts| parts.iter().filter_map(|p| p["text"].as_str()).collect::<Vec<_>>().join("\n"))
        .unwrap_or_default();
    if text.trim().is_empty() {
        return Err("the answer was empty".into());
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_mails_long_numbers_and_sensitive_columns() {
        assert_eq!(redact_value("a@b.co").0, "[redacted]");
        assert_eq!(redact_value("call 13800138000 now").0, "call [redacted] now");
        assert_eq!(redact_value("plain text").0, "plain text");
        assert!(sensitive_column("contactEmail") && sensitive_column("payer_phone"));
        assert!(!sensitive_column("status"));
    }

    #[test]
    fn sample_is_limited_and_redacted() {
        let input = Input {
            executed: "SELECT 1".into(),
            description: String::new(),
            columns: vec!["id".into(), "contact_email".into()],
            rows: (0..30).map(|i| vec![Some(i.to_string()), Some("x@y.z".into())]).collect(),
            total_rows: 30,
            truncated: false,
            question: "q".into(),
            doc_paths: vec![],
        };
        let (text, redacted) = sample_text(&input);
        assert_eq!(text.lines().count(), 1 + SAMPLE_ROWS);
        assert!(!text.contains("x@y.z"));
        assert_eq!(redacted, SAMPLE_ROWS);
    }

    #[test]
    fn models_are_the_offered_ones() {
        assert!(MODELS.contains(&"claude-sonnet-5-5"));
    }
}
