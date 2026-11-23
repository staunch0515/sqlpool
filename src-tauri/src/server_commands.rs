//! Saved commands run on a server over ssh. A command is a shell command the user writes and saves, like
//! a saved SQL statement: it can be filed in a group, has `:name` parameters, and its output is a result
//! (the bottom panel, export, keep). The command runs on the server's login shell, so it is written with
//! the same care as any shell script: parameter values are quoted, and the output is masked for secrets.

use crate::db::QueryResult;
use std::collections::BTreeMap;
use std::process::Command;
use std::time::Instant;

/// Names of `:name` parameters in a command, in order of first use. `::` and words after a letter or
/// digit (such as `http://`, `a:b`) are not parameters.
pub fn used_params(command: &str) -> Vec<String> {
    let chars: Vec<char> = command.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev_ok = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == ':');
        if c == ':' && prev_ok && i + 1 < chars.len() && (chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '_') {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let name: String = chars[i + 1..j].iter().collect();
            if !out.contains(&name) {
                out.push(name);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Replaces each `:name` with its value, quoted for the shell. An empty or missing value is an empty string.
pub fn render(command: &str, params: &BTreeMap<String, Option<String>>) -> String {
    substitute(command, |name| shell_quote(&params.get(name).cloned().flatten().unwrap_or_default()))
}

/// Replaces each `:name` (same rule as `used_params`) with what `value` returns for the name.
pub fn substitute(command: &str, value: impl Fn(&str) -> String) -> String {
    let chars: Vec<char> = command.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev_ok = i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_' || chars[i - 1] == ':');
        if c == ':' && prev_ok && i + 1 < chars.len() && (chars[i + 1].is_ascii_alphabetic() || chars[i + 1] == '_') {
            let mut j = i + 1;
            while j < chars.len() && (chars[j].is_ascii_alphanumeric() || chars[j] == '_') {
                j += 1;
            }
            let name: String = chars[i + 1..j].iter().collect();
            out.push_str(&value(&name));
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// A value as one shell word: single quotes, with each `'` written as `'\''`.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Hides the values of variables whose names say they are secrets. Works on `NAME=value` lines.
/// A value the command already reduced to a length, such as `(length: 12)`, is kept.
pub fn mask(text: &str) -> String {
    const SECRET_WORDS: [&str; 6] = ["PASS", "SECRET", "TOKEN", "KEY", "CREDENTIAL", "PWD"];
    let mut out = String::new();
    for line in text.lines() {
        let masked = match line.split_once('=') {
            Some((name, value))
                if SECRET_WORDS.iter().any(|w| name.to_ascii_uppercase().contains(w)) && !value.starts_with("(length:") =>
            {
                format!("{name}=********")
            }
            _ => line.to_string(),
        };
        out.push_str(&masked);
        out.push('\n');
    }
    out
}

/// An ssh alias: letters, digits and `._-@:`, not starting with `-`.
pub fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 128
        && !host.starts_with('-')
        && host.chars().all(|c| c.is_ascii_alphanumeric() || "._-@:".contains(c))
}

/// Runs a rendered command on the host. The result has one row per output line: `stream` (stdout or
/// stderr) and `line`. Output is masked and limited to `max_rows`.
pub fn run(host: &str, rendered: &str, max_rows: usize) -> Result<QueryResult, String> {
    if !host_ok(host) {
        return Err(format!("the ssh host is not allowed: {host}"));
    }
    let started = Instant::now();
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host, rendered])
        .output()
        .map_err(|e| format!("ssh: {e}"))?;

    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut truncated = false;
    let mut push = |stream: &str, text: &str| {
        for line in mask(text).lines() {
            if rows.len() >= max_rows {
                truncated = true;
                return;
            }
            rows.push(vec![Some(stream.to_string()), Some(line.to_string())]);
        }
    };
    push("stdout", &String::from_utf8_lossy(&out.stdout));
    push("stderr", &String::from_utf8_lossy(&out.stderr));
    if rows.is_empty() {
        rows.push(vec![Some("info".into()), Some(format!("no output; exit status {}", out.status))]);
    }
    if !out.status.success() {
        rows.push(vec![Some("info".into()), Some(format!("exit status {}", out.status))]);
    }
    Ok(QueryResult {
        columns: vec!["stream".into(), "line".into()],
        rows,
        truncated,
        elapsed_ms: started.elapsed().as_millis(),
        executed_sql: format!("ssh {host} {rendered}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_params_and_skips_urls_and_double_colons() {
        assert_eq!(used_params("curl :url -H :token"), vec!["url", "token"]);
        assert!(used_params("echo http://x:8080 a::b").is_empty());
        assert_eq!(used_params("echo :a :a"), vec!["a"]);
    }

    #[test]
    fn renders_quoted_values() {
        let mut p = BTreeMap::new();
        p.insert("name".to_string(), Some("it's ok".to_string()));
        p.insert("empty".to_string(), None);
        assert_eq!(render("echo :name :empty http://h", &p), "echo 'it'\\''s ok' '' http://h");
        assert_eq!(shell_quote("a; rm -rf /"), "'a; rm -rf /'");
    }

    #[test]
    fn masks_secret_values_only() {
        let m = mask("SPRING_MAIL_HOST=smtp.example.com\nSPRING_MAIL_PASSWORD=(length: 12)\nJABIZ_MAIL_API_TOKEN=abc123\n");
        assert!(m.contains("SPRING_MAIL_HOST=smtp.example.com"));
        assert!(m.contains("SPRING_MAIL_PASSWORD=(length: 12)"));
        assert!(m.contains("JABIZ_MAIL_API_TOKEN=********"));
        assert!(!m.contains("abc123"));
    }

    #[test]
    fn host_is_checked() {
        assert!(host_ok("quizbuks-staging"));
        assert!(!host_ok("-oProxyCommand=x") && !host_ok("a b") && !host_ok(""));
    }
}
