//! Back up a database on a server reached by ssh, bring the dump here, and restore it into a local
//! Docker container as a new database.
//!
//! Remote: `docker exec CONTAINER pg_dump -U postgres -Fc DB > /root/DB-DATE.dump` (the same as the
//! server's own backup). Local: the dump is copied to the app's backups folder, copied into the local
//! container, and restored into a new database (the existing databases are never touched).

use std::path::{Path, PathBuf};
use std::process::Command;

/// A name that is safe to put in a shell command or a docker argument.
pub fn name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && !s.starts_with('-')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
}

/// Docker is usually in /usr/local/bin or Homebrew's bin; a GUI app does not inherit the shell's PATH.
pub fn docker_bin() -> String {
    for p in [
        "/usr/local/bin/docker",
        "/opt/homebrew/bin/docker",
        "/Applications/Docker.app/Contents/Resources/bin/docker",
    ] {
        if Path::new(p).exists() {
            return p.to_string();
        }
    }
    "docker".to_string()
}

/// The local date, as `date` prints it: `2026-11-12` (the server's file naming) and `20261112` (the restore name).
pub fn today() -> Result<(String, String), String> {
    let out = Command::new("date").arg("+%F %Y%m%d").output().map_err(|e| format!("date: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut parts = text.split(' ');
    match (parts.next(), parts.next()) {
        (Some(a), Some(b)) => Ok((a.to_string(), b.to_string())),
        _ => Err("could not read the date".into()),
    }
}

/// A folder on the server: `~` (the login user's home) or an absolute path, with safe characters only.
pub fn remote_dir_ok(dir: &str) -> bool {
    !dir.is_empty()
        && dir.len() <= 200
        && (dir == "~" || dir.starts_with('/') || dir.starts_with("~/"))
        && !dir.contains("..")
        && dir.chars().all(|c| c.is_ascii_alphanumeric() || "_-./~".contains(c))
}

/// The dump's path on the server: `DIR/DB-DATE.dump`, named as the server's own backup names it.
pub fn remote_dump_path(dir: &str, db: &str, date: &str) -> String {
    format!("{}/{db}-{date}.dump", dir.trim_end_matches('/'))
}

fn ssh(host: &str, command: &str) -> Result<Vec<u8>, String> {
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", host, command])
        .output()
        .map_err(|e| format!("ssh: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("ssh exited with {}", out.status) } else { err });
    }
    Ok(out.stdout)
}

fn docker(args: &[&str]) -> Result<Vec<u8>, String> {
    let out = Command::new(docker_bin()).args(args).output().map_err(|e| format!("docker: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("docker exited with {}", out.status) } else { err });
    }
    Ok(out.stdout)
}

pub struct Plan<'a> {
    pub ssh_host: &'a str,
    /// Where the dump is written on the server: `~` (the login user's home) or an absolute folder.
    pub remote_dir: &'a str,
    pub remote_container: &'a str,
    pub remote_db: &'a str,
    pub local_container: &'a str,
    pub local_db: &'a str,
    /// Where the downloaded dump is kept (the app's backups folder).
    pub backups_dir: PathBuf,
}

pub struct Outcome {
    pub dump_path: PathBuf,
    pub bytes: usize,
    /// What pg_restore printed. Empty when the restore was clean.
    pub warnings: String,
}

/// Runs the whole backup: dump on the server, download, restore into a new local database.
pub fn run(plan: &Plan) -> Result<Outcome, String> {
    for (label, v) in [
        ("ssh host", plan.ssh_host),
        ("remote container", plan.remote_container),
        ("remote database", plan.remote_db),
        ("local container", plan.local_container),
        ("local database", plan.local_db),
    ] {
        if !name_ok(v) {
            return Err(format!("{label} has characters that are not allowed: {v}"));
        }
    }
    if !remote_dir_ok(plan.remote_dir) {
        return Err(format!("the server folder is not allowed: {}", plan.remote_dir));
    }
    let (date, _) = today()?;
    let remote_file = remote_dump_path(plan.remote_dir, plan.remote_db, &date);

    // 1. dump on the server, the same command as the server's own backup
    ssh(
        plan.ssh_host,
        &format!(
            "docker exec {} pg_dump -U postgres -Fc {} > {}",
            plan.remote_container, plan.remote_db, remote_file
        ),
    )?;

    // 2. download it
    let bytes = ssh(plan.ssh_host, &format!("cat {remote_file}"))?;
    if bytes.is_empty() {
        return Err(format!("the dump {remote_file} on the server is empty"));
    }
    std::fs::create_dir_all(&plan.backups_dir).map_err(|e| format!("backups folder: {e}"))?;
    let dump_path = plan.backups_dir.join(format!("{}-{date}.dump", plan.remote_db));
    std::fs::write(&dump_path, &bytes).map_err(|e| format!("{}: {e}", dump_path.display()))?;

    // 3. a new database in the local container, restored from the dump
    let tmp = "/tmp/sqlpool-restore.dump";
    let local_path = dump_path.to_string_lossy().to_string();
    docker(&["cp", &local_path, &format!("{}:{tmp}", plan.local_container)])?;
    let created = docker(&["exec", plan.local_container, "createdb", "-U", "postgres", plan.local_db]);
    if let Err(e) = created {
        let _ = docker(&["exec", plan.local_container, "rm", "-f", tmp]);
        return Err(format!("could not create the database {}: {e}", plan.local_db));
    }
    let restore = Command::new(docker_bin())
        .args([
            "exec",
            plan.local_container,
            "pg_restore",
            "-U",
            "postgres",
            "-d",
            plan.local_db,
            "--no-owner",
            "--no-privileges",
            tmp,
        ])
        .output()
        .map_err(|e| format!("docker: {e}"))?;
    let _ = docker(&["exec", plan.local_container, "rm", "-f", tmp]);
    let warnings = String::from_utf8_lossy(&restore.stderr).trim().to_string();
    if !restore.status.success() && warnings.is_empty() {
        return Err(format!("pg_restore exited with {}", restore.status));
    }

    Ok(Outcome { dump_path, bytes: bytes.len(), warnings })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths() {
        assert!(name_ok("quizbuks_staging"));
        assert!(name_ok("quizbuks-staging"));
        assert!(!name_ok("db; rm -rf /"));
        assert!(!name_ok("-x"));
        assert!(!name_ok(""));
        assert_eq!(
            remote_dump_path("~", "quizbuks_staging", "2026-11-12"),
            "~/quizbuks_staging-2026-11-12.dump"
        );
        assert_eq!(remote_dump_path("/root/", "db", "2026-11-12"), "/root/db-2026-11-12.dump");
        assert!(remote_dir_ok("~") && remote_dir_ok("/home/deploy/backups"));
        assert!(!remote_dir_ok("/x; rm -rf /") && !remote_dir_ok("../etc") && !remote_dir_ok("relative"));
        let (d, n) = today().unwrap();
        assert_eq!(d.len(), 10);
        assert_eq!(n.len(), 8);
    }
}
