//! Connection strings for a data source, and the reverse for a URL typed or saved before data sources existed.

use crate::store::Datasource;

/// The string `db::run_readonly` takes: a PostgreSQL URL for a direct connection, or the ssh form
/// `ssh://HOST/CONTAINER?user=USER&db=DATABASE` when the query runs on the server (see remote.rs).
pub fn for_datasource(ds: &Datasource, password: &str) -> String {
    if ds.via_ssh {
        format!("ssh://{}/{}?user={}&db={}", ds.ssh_host, ds.container, ds.user, ds.database)
    } else {
        postgres_url(&ds.host, ds.port, &ds.database, &ds.user, password)
    }
}

/// A direct `postgres://` URL. The user, password and database name are percent-encoded.
pub fn postgres_url(host: &str, port: i64, database: &str, user: &str, password: &str) -> String {
    format!("postgres://{}:{}@{}:{}/{}", encode(user), encode(password), host, port, encode(database))
}

/// The parts of a `postgres://` URL.
#[derive(Debug, PartialEq)]
pub struct PgUrl {
    pub host: String,
    pub port: i64,
    pub database: String,
    pub user: String,
    pub password: String,
}

pub fn parse_pg_url(url: &str) -> Result<PgUrl, String> {
    let rest = url
        .trim()
        .strip_prefix("postgres://")
        .or_else(|| url.trim().strip_prefix("postgresql://"))
        .ok_or("expected postgres://user:password@host:port/database")?;
    let (userinfo, hostpath) = rest.rsplit_once('@').ok_or("expected user:password@host")?;
    let (user, password) = match userinfo.split_once(':') {
        Some((u, p)) => (decode(u)?, decode(p)?),
        None => (decode(userinfo)?, String::new()),
    };
    let (hostport, path) = hostpath.split_once('/').unwrap_or((hostpath, ""));
    let database = path.split('?').next().unwrap_or("");
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h.to_string(), p.parse::<i64>().map_err(|e| e.to_string())?)
        }
        _ => (hostport.to_string(), 5432),
    };
    if host.is_empty() {
        return Err("the URL has no host".into());
    }
    if database.is_empty() {
        return Err("the URL has no database name".into());
    }
    Ok(PgUrl {
        host,
        port,
        database: decode(database)?,
        user,
        password,
    })
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn decode(s: &str) -> Result<String, String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3).ok_or("bad percent-encoding in the URL")?;
            let v = u8::from_str_radix(hex, 16).map_err(|_| "bad percent-encoding in the URL".to_string())?;
            out.push(v);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "the URL is not valid UTF-8".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_postgres_url_with_encoded_password() {
        let u = parse_pg_url("postgres://app%40x:p%23w@db.example:6543/shop?sslmode=require").unwrap();
        assert_eq!(
            u,
            PgUrl {
                host: "db.example".into(),
                port: 6543,
                database: "shop".into(),
                user: "app@x".into(),
                password: "p#w".into(),
            }
        );
        assert_eq!(parse_pg_url("postgres://u@h/d").unwrap().port, 5432);
        assert!(parse_pg_url("postgres://u:p@h/").is_err());
        assert!(parse_pg_url("mysql://u:p@h/d").is_err());
    }

    #[test]
    fn builds_the_string_for_direct_and_ssh_data_sources() {
        let ds = |via_ssh| Datasource {
            id: 1,
            name: "n".into(),
            host: "db.example".into(),
            port: 5432,
            database: "shop".into(),
            user: "app".into(),
            via_ssh,
            ssh_host: "quizbuks-staging".into(),
            container: "postgres".into(),
            password_saved: true,
        };
        assert_eq!(for_datasource(&ds(false), "p#w"), "postgres://app:p%23w@db.example:5432/shop");
        assert_eq!(
            for_datasource(&ds(true), "ignored"),
            "ssh://quizbuks-staging/postgres?user=app&db=shop"
        );
    }
}
