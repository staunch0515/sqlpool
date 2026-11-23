//! Local bash scripts: scripts the user keeps in the app and runs on this Mac. A script is stored in the
//! app database (like saved SQL and server commands) and can be exported to `.sh` files in a folder.
//! Its `:name` parameters are filled the same way as a server command's: from the active environment,
//! then from the form, quoted for the shell. The output is a result like the other runners: one row per
//! line, masked for secrets, and limited.

use crate::db::QueryResult;
use crate::server_commands::{mask, substitute, used_params};
use crate::store::LocalScript;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How long a script may run before it is stopped. Long enough for a build and a start (the local demo).
pub const TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Most output lines kept in the result.
const MAX_ROWS: usize = 5_000;

/// Runs a script with bash and returns its output. `script` is the text after the parameters are filled
/// in; the result keeps it as the executed text. It is written to a temporary file so the script can read
/// its own standard input.
pub fn run(script: &str, timeout: Duration) -> Result<QueryResult, String> {
    let started = Instant::now();
    let path = std::env::temp_dir().join(format!("sqlpool-script-{}.sh", nanos()));
    std::fs::write(&path, script).map_err(|e| format!("write the script: {e}"))?;

    let spawned = Command::new("/bin/bash")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PATH", path_for_gui_app())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(format!("bash: {e}"));
        }
    };
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_reader = std::thread::spawn(move || read_all(out_pipe));
    let err_reader = std::thread::spawn(move || read_all(err_pipe));

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = std::fs::remove_file(&path);
                return Err(format!("wait for the script: {e}"));
            }
        }
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    let _ = std::fs::remove_file(&path);

    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut truncated = false;
    let mut push = |stream: &str, text: &[u8]| {
        for line in mask(&String::from_utf8_lossy(text)).lines() {
            if rows.len() >= MAX_ROWS {
                truncated = true;
                return;
            }
            rows.push(vec![Some(stream.to_string()), Some(line.to_string())]);
        }
    };
    push("stdout", &stdout);
    push("stderr", &stderr);
    match status {
        None => rows.push(vec![Some("info".into()), Some(format!("stopped after {} s (timeout)", timeout.as_secs()))]),
        Some(s) if !s.success() => rows.push(vec![Some("info".into()), Some(format!("{s}"))]),
        Some(_) if rows.is_empty() => rows.push(vec![Some("info".into()), Some("no output; exit status 0".into())]),
        Some(_) => {}
    }
    Ok(QueryResult {
        columns: vec!["stream".into(), "line".into()],
        rows,
        truncated,
        elapsed_ms: started.elapsed().as_millis(),
        executed_sql: script.to_string(),
    })
}

fn read_all(pipe: Option<impl Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_end(&mut buf);
    }
    buf
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

/// An app started from Finder gets a short PATH. Adds the usual places for Homebrew and Docker.
fn path_for_gui_app() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    let base = if current.is_empty() { "/usr/bin:/bin:/usr/sbin:/sbin".to_string() } else { current };
    format!("/opt/homebrew/bin:/usr/local/bin:{base}")
}

/// The script as a standalone `.sh` file: a header with the name and description, the variables it reads
/// from the environment, then the body. Each `:name` becomes `"${name}"`, so the file reads the environment
/// when it runs.
pub fn export_text(script: &LocalScript) -> String {
    let mut out = String::from("#!/usr/bin/env bash\n");
    out.push_str(&format!("# {}\n", script.name.replace(['\n', '\r'], " ")));
    for line in script.description.lines().filter(|l| !l.trim().is_empty()) {
        out.push_str(&format!("# {line}\n"));
    }
    let names = used_params(&script.body);
    if !names.is_empty() {
        out.push_str(&format!("# Reads these variables from the environment: {}\n", names.join(", ")));
    }
    out.push('\n');
    let body = substitute(&script.body, |name| format!("\"${{{name}}}\""));
    out.push_str(&body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// A file name from a script name: letters, digits, space, `.`, `_` and `-`.
pub fn file_name(name: &str) -> String {
    let clean: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || " ._-".contains(c) { c } else { '-' })
        .collect();
    let clean = clean.trim_matches(|c| c == '.' || c == ' ' || c == '-').to_string();
    if clean.is_empty() { "script".to_string() } else { clean }
}

/// Writes every script into `dir`: one folder per group (scripts with no group sit in `dir`). A file
/// name used twice in a folder gets the script's id added. Returns how many files were written.
pub fn export_all(dir: &str, scripts: &[LocalScript]) -> Result<usize, String> {
    use std::os::unix::fs::PermissionsExt;
    let root = std::path::Path::new(dir);
    let mut seen: HashSet<std::path::PathBuf> = HashSet::new();
    let mut count = 0;
    for s in scripts {
        let text = export_text(s);
        let folder = if s.group.trim().is_empty() { root.to_path_buf() } else { root.join(file_name(&s.group)) };
        std::fs::create_dir_all(&folder).map_err(|e| format!("create {}: {e}", folder.display()))?;
        let mut path = folder.join(format!("{}.sh", file_name(&s.name)));
        if seen.contains(&path) {
            path = folder.join(format!("{} {}.sh", file_name(&s.name), s.id));
        }
        let mut f = std::fs::File::create(&path).map_err(|e| format!("write {}: {e}", path.display()))?;
        f.write_all(text.as_bytes()).map_err(|e| format!("write {}: {e}", path.display()))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("chmod {}: {e}", path.display()))?;
        seen.insert(path);
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn script(name: &str, group: &str, body: &str) -> LocalScript {
        LocalScript {
            id: 7,
            name: name.into(),
            group: group.into(),
            description: "Line one\nLine two".into(),
            body: body.into(),
            updated_at: 0,
        }
    }

    #[test]
    fn runs_the_script_and_masks_secrets() {
        let r = run("echo hello\necho \"DB_PASSWORD=abc\"\n", Duration::from_secs(10)).unwrap();
        assert_eq!(r.rows[0][1].as_deref(), Some("hello"));
        assert_eq!(r.rows[1][1].as_deref(), Some("DB_PASSWORD=********"));
    }

    #[test]
    fn reports_exit_status_and_stderr() {
        let r = run("echo oops >&2\nexit 3\n", Duration::from_secs(10)).unwrap();
        assert!(r.rows.iter().any(|row| row[0].as_deref() == Some("stderr") && row[1].as_deref() == Some("oops")));
        assert!(r.rows.iter().any(|row| row[1].as_deref().is_some_and(|t| t.starts_with("exit status"))));
    }

    #[test]
    fn stops_after_the_timeout() {
        let r = run("sleep 5\n", Duration::from_millis(300)).unwrap();
        assert!(r.rows.iter().any(|row| row[1].as_deref().is_some_and(|t| t.contains("timeout"))));
    }

    #[test]
    fn export_reads_parameters_from_the_environment() {
        let text = export_text(&script("Disk", "ops", "df -h :dir"));
        assert!(text.starts_with("#!/usr/bin/env bash\n# Disk\n# Line one\n# Line two\n# Reads these variables from the environment: dir\n"));
        assert!(text.ends_with("df -h \"${dir}\"\n"));
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_name("Disk usage / staging"), "Disk usage - staging");
        assert_eq!(file_name("  ../etc  "), "etc");
        assert_eq!(file_name("***"), "script");
    }

    #[test]
    fn exports_into_group_folders() {
        let dir = std::env::temp_dir().join(format!("sqlpool-export-test-{}", nanos()));
        let dir_s = dir.to_string_lossy().to_string();
        let scripts = vec![script("Disk", "ops", "df -h"), script("Disk", "ops", "uptime"), script("Top", "", "ls")];
        assert_eq!(export_all(&dir_s, &scripts).unwrap(), 3);
        assert!(dir.join("ops").join("Disk.sh").exists());
        assert!(dir.join("ops").join("Disk 7.sh").exists());
        assert!(dir.join("Top.sh").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
