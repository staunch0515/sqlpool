//! Queries that the versioned (time-versioned) tables allow. A versioned table only gets INSERTs: a change is
//! a new version (version_no goes up), a deletion is a new version marked deleted (is_deleted). So the current
//! state, the history of a record, the state at a moment and the changes in a period can all be read directly.

use std::collections::BTreeMap;

/// The versioned columns of a table, as physical names.
pub struct Versioned<'a> {
    pub table: &'a str,
    pub pk: &'a str,
    pub version_no: &'a str,
    pub effect_start: &'a str,
    pub created: &'a str,
    pub deleted: &'a str,
}

/// A query a versioned table allows, with the parameters it needs.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct QueryDef {
    pub id: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    /// (name, label, required)
    pub params: Vec<ParamDef>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
pub struct ParamDef {
    pub name: &'static str,
    pub label: &'static str,
    pub required: bool,
}

pub fn definitions() -> Vec<QueryDef> {
    vec![
        QueryDef {
            id: "current",
            title: "Current versions",
            description: "The latest version of every record that is not deleted: what the table holds now.",
            params: vec![],
        },
        QueryDef {
            id: "history",
            title: "History of a record",
            description: "Every version of one record, oldest first.",
            params: vec![ParamDef { name: "id", label: "Primary key", required: true }],
        },
        QueryDef {
            id: "asOf",
            title: "State at a time",
            description: "What each record looked like at the given time: the version in effect then.",
            params: vec![ParamDef { name: "asOf", label: "At (UTC, e.g. 2026-11-01 12:00)", required: false }],
        },
        QueryDef {
            id: "changes",
            title: "Changes in a period",
            description: "Every version written in the period, by the time it was recorded.",
            params: vec![
                ParamDef { name: "from", label: "From (UTC)", required: true },
                ParamDef { name: "to", label: "To (UTC)", required: true },
            ],
        },
        QueryDef {
            id: "deleted",
            title: "Deleted records",
            description: "Records whose latest version is a deletion.",
            params: vec![],
        },
    ]
}

/// A value as a SQL literal: quoted, with each quote doubled.
pub fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// The column list of a SELECT: plain column names, at least one.
pub fn column_list(columns: &[String]) -> Result<String, String> {
    if columns.is_empty() {
        return Err("select at least one column".into());
    }
    for c in columns {
        if c.is_empty() || !c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return Err(format!("not a column name: {c}"));
        }
    }
    Ok(columns.join(", "))
}

/// Builds the SQL for a query. Parameters are checked: the required ones must be given. `columns`, when
/// given, replaces `*` in the outer SELECT: only those columns come back.
pub fn build(
    v: &Versioned,
    id: &str,
    params: &BTreeMap<String, String>,
    columns: Option<&[String]>,
) -> Result<String, String> {
    let sql = build_all(v, id, params)?;
    match columns {
        None => Ok(sql),
        Some(cols) => {
            let list = column_list(cols)?;
            // The outer SELECT is the first `SELECT *` of every query.
            Ok(sql.replacen("SELECT *", &format!("SELECT {list}"), 1))
        }
    }
}

fn build_all(v: &Versioned, id: &str, params: &BTreeMap<String, String>) -> Result<String, String> {
    if !v.table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("not a table name: {}", v.table));
    }
    let def = definitions()
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| format!("unknown query: {id}"))?;
    let mut given = BTreeMap::new();
    for p in &def.params {
        match params.get(p.name).map(|s| s.trim()).filter(|s| !s.is_empty()) {
            Some(value) => {
                given.insert(p.name, value.to_string());
            }
            None if p.required => return Err(format!("{} is required", p.label)),
            None => {}
        }
    }
    let t = v.table;
    let (pk, ver, eff, created, del) = (v.pk, v.version_no, v.effect_start, v.created, v.deleted);
    // The latest version of each record, by version number.
    let latest = format!("SELECT DISTINCT ON ({pk}) * FROM {t} ORDER BY {pk}, {ver} DESC");
    Ok(match id {
        "current" => format!("SELECT * FROM ({latest}) v WHERE NOT {del} ORDER BY {pk}"),
        "history" => format!(
            "SELECT * FROM {t} WHERE {pk} = {} ORDER BY {ver}",
            literal(given["id"].as_str())
        ),
        "asOf" => {
            let at = match given.get("asOf") {
                Some(a) => literal(a),
                None => "now()".to_string(),
            };
            // The version in effect at the time: the last one that took effect by then.
            format!(
                "SELECT * FROM (SELECT DISTINCT ON ({pk}) * FROM {t} WHERE {eff} <= {at} \
                 ORDER BY {pk}, {eff} DESC, {ver} DESC) v WHERE NOT {del} ORDER BY {pk}"
            )
        }
        "changes" => format!(
            "SELECT * FROM {t} WHERE {created} >= {} AND {created} < {} ORDER BY {created}, {pk}, {ver}",
            literal(given["from"].as_str()),
            literal(given["to"].as_str())
        ),
        "deleted" => format!("SELECT * FROM ({latest}) v WHERE {del} ORDER BY {pk}"),
        _ => return Err(format!("unknown query: {id}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v() -> Versioned<'static> {
        Versioned {
            table: "customer_version",
            pk: "customer_id",
            version_no: "version_no",
            effect_start: "effect_start_time",
            created: "created_time",
            deleted: "is_deleted",
        }
    }

    fn p(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_column_list_replaces_the_outer_star() {
        let cols = vec!["customer_id".to_string(), "customer_name".to_string()];
        let sql = build(&v(), "current", &p(&[]), Some(&cols)).unwrap();
        assert!(sql.starts_with("SELECT customer_id, customer_name FROM (SELECT DISTINCT ON"));
        assert!(build(&v(), "current", &p(&[]), Some(&[])).is_err());
        assert!(build(&v(), "current", &p(&[]), Some(&["a; drop".to_string()])).is_err());
        assert!(build(&v(), "history", &p(&[("id", "1")]), None).unwrap().starts_with("SELECT * FROM"));
    }

    #[test]
    fn current_and_deleted_use_the_latest_version() {
        let sql = build(&v(), "current", &p(&[]), None).unwrap();
        assert!(sql.contains("DISTINCT ON (customer_id) * FROM customer_version ORDER BY customer_id, version_no DESC"));
        assert!(sql.contains("WHERE NOT is_deleted"));
        assert!(build(&v(), "deleted", &p(&[]), None).unwrap().contains("WHERE is_deleted"));
    }

    #[test]
    fn history_needs_the_key_and_quotes_it() {
        assert!(build(&v(), "history", &p(&[]), None).is_err());
        let sql = build(&v(), "history", &p(&[("id", "o'neil")]), None).unwrap();
        assert!(sql.contains("customer_id = 'o''neil' ORDER BY version_no"));
    }

    #[test]
    fn as_of_defaults_to_now_and_changes_need_a_period() {
        assert!(build(&v(), "asOf", &p(&[]), None).unwrap().contains("effect_start_time <= now()"));
        assert!(build(&v(), "asOf", &p(&[("asOf", "2026-11-01 12:00")]), None).unwrap().contains("<= '2026-11-01 12:00'"));
        assert!(build(&v(), "changes", &p(&[("from", "2026-11-01")]), None).is_err());
        let sql = build(&v(), "changes", &p(&[("from", "2026-11-01"), ("to", "2026-11-02")]), None).unwrap();
        assert!(sql.contains("created_time >= '2026-11-01' AND created_time < '2026-11-02'"));
    }

    #[test]
    fn only_plain_table_names() {
        let bad = Versioned { table: "x; drop table y", ..v() };
        assert!(build(&bad, "current", &p(&[]), None).is_err());
    }
}
