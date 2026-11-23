//! Parsing and rendering of catalog query files.
//!
//! A file looks like:
//!
//! ```text
//! /*---
//! id: qb.sponsor.analytics.trend
//! params:
//!   from: { kind: { type: temporal }, required: true }
//! ---*/
//! SELECT ... {{QbStatsRun.zone}} ... WHERE x >= :from
//! ```
//!
//! The front matter is YAML inside a block comment. The body contains
//! `{{Entity.field}}` placeholders (resolved through a mapping file) and
//! `:name` parameters (bound from the form).

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Kind {
    #[serde(rename = "type", default)]
    pub ty: String,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default, rename = "maxLength")]
    pub max_length: Option<u32>,
    #[serde(default)]
    pub precision: Option<u32>,
    #[serde(default)]
    pub scale: Option<u32>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default, rename = "enum")]
    pub enum_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ParamSpec {
    #[serde(default)]
    pub like: Option<String>,
    #[serde(default)]
    pub kind: Option<Kind>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FrontMatter {
    pub id: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub entities: Vec<String>,
    #[serde(default)]
    pub datasets: BTreeMap<String, String>,
    /// Keeps the declaration order from the YAML, which is the form order.
    #[serde(default)]
    pub params: IndexMap<String, ParamSpec>,
    #[serde(default)]
    pub results: serde_json::Value,
    #[serde(default)]
    pub list: serde_json::Value,
    #[serde(default)]
    pub permissions: Vec<String>,
    /// Which parameters give the time of a temporal read (docs/design/05 section 2.1).
    #[serde(default, rename = "timeSlice")]
    pub time_slice: Option<TimeSliceSpec>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TimeSliceSpec {
    /// Parameter with the business time; empty or absent means now.
    #[serde(default, rename = "asOf")]
    pub as_of: Option<String>,
    /// Parameter with the recording time; empty or absent means no limit.
    #[serde(default, rename = "knownAt")]
    pub known_at: Option<String>,
}

/// All `.sql` files under `dir`, recursively, sorted.
pub fn collect_sql_files(dir: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else if path.extension().is_some_and(|e| e == "sql") {
                out.push(path);
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, &mut out)?;
    out.sort();
    Ok(out)
}

/// Splits a file into its front matter and SQL body.
pub fn parse_source(src: &str) -> Result<(FrontMatter, String), String> {
    let rest = src
        .trim_start()
        .strip_prefix("/*---")
        .ok_or("missing front matter (expected '/*---' at the top)")?;
    let end = rest
        .find("---*/")
        .ok_or("front matter is not closed (expected '---*/')")?;
    let front: FrontMatter =
        serde_yaml_ng::from_str(&rest[..end]).map_err(|e| format!("front matter: {e}"))?;
    let body = rest[end + "---*/".len()..]
        .trim_start_matches(['\r', '\n'])
        .to_string();
    Ok((front, body))
}

/// A temporal entity in the mapping: its table, primary key and system columns.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemporalEntity {
    pub table: String,
    #[serde(default)]
    pub pk: Option<String>,
    #[serde(default)]
    pub write_once: bool,
    /// Logical system field -> physical column (e.g. "versionNo" -> "version_no").
    #[serde(default)]
    pub columns: BTreeMap<String, String>,
}

/// The mapping file: plain names (`"Entity.field": "column"`, `"Entity": "table"`) and temporal entities.
#[derive(Debug, Clone, Default)]
pub struct Mapping {
    pub tables: BTreeMap<String, String>,
    pub temporal: BTreeMap<String, TemporalEntity>,
}

/// Reads the mapping JSON. A string value is a plain name; an object value is a temporal entity.
pub fn parse_mapping(text: &str) -> Result<Mapping, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let obj = value.as_object().ok_or("mapping must be a JSON object")?;
    let mut mapping = Mapping::default();
    for (key, v) in obj {
        match v {
            serde_json::Value::String(s) => {
                mapping.tables.insert(key.clone(), s.clone());
            }
            serde_json::Value::Object(_) => {
                let entity: TemporalEntity =
                    serde_json::from_value(v.clone()).map_err(|e| format!("mapping {key}: {e}"))?;
                mapping.temporal.insert(key.clone(), entity);
            }
            _ => return Err(format!("mapping {key}: expected a string or an object")),
        }
    }
    Ok(mapping)
}

/// SQL for the time of a temporal read: `asOf` and optional `knownAt`, already rendered as SQL.
#[derive(Debug, Clone)]
pub struct TimeSql {
    pub as_of: String,
    pub known_at: Option<String>,
}

impl Default for TimeSql {
    fn default() -> Self {
        TimeSql { as_of: "now()".to_string(), known_at: None }
    }
}

/// Time of a read from the form values. An empty or missing value means now (asOf) or no limit (knownAt).
pub fn time_sql(
    spec: &Option<TimeSliceSpec>,
    values: &BTreeMap<String, Option<String>>,
) -> Result<TimeSql, String> {
    let Some(spec) = spec else { return Ok(TimeSql::default()) };
    let value_of = |name: &str| values.get(name).cloned().flatten();
    let as_of = match spec.as_of.as_deref().and_then(|n| value_of(n).map(|v| (n, v))) {
        Some((_, v)) => format!("CAST({} AS timestamptz)", sql_literal(Some(&v))?),
        None => "now()".to_string(),
    };
    let known_at = match spec.known_at.as_deref().and_then(|n| value_of(n).map(|v| (n, v))) {
        Some((_, v)) => Some(format!("CAST({} AS timestamptz)", sql_literal(Some(&v))?)),
        None => None,
    };
    Ok(TimeSql { as_of, known_at })
}

/// Time of a run. A query with a `timeSlice` takes it from its parameters. A query without one
/// (reads temporal entities) takes the optional "As of" value given by the user; empty means now.
pub fn run_time(
    spec: &Option<TimeSliceSpec>,
    values: &BTreeMap<String, Option<String>>,
    as_of: Option<&str>,
) -> Result<TimeSql, String> {
    match (spec, as_of.filter(|v| !v.trim().is_empty())) {
        (None, Some(v)) => Ok(TimeSql {
            as_of: format!("CAST({} AS timestamptz)", sql_literal(Some(v))?),
            known_at: None,
        }),
        _ => time_sql(spec, values),
    }
}

/// Does the body read a temporal entity?
pub fn uses_temporal(body: &str, mapping: &Mapping) -> bool {
    placeholder_names(body).iter().any(|n| mapping.temporal.contains_key(n))
}

fn ident(name: &str) -> Result<&str, String> {
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        Ok(name)
    } else {
        Err(format!("mapping has an invalid identifier: {name}"))
    }
}

/// `(SELECT v.* FROM (...) v WHERE NOT v.is_deleted)`: the versions in effect at the time, without tombstones.
/// Mirrors QueryCompiler.source / templateExpression (docs/design/04 section 5.1, decision D3).
pub fn temporal_source(entity: &TemporalEntity, time: &TimeSql) -> Result<String, String> {
    let pk = ident(entity.pk.as_deref().ok_or("temporal entity has no primary key column")?)?;
    let table = ident(&entity.table)?;
    let col = |logical: &str, fallback: &str| -> Result<String, String> {
        let physical = entity.columns.get(logical).map(String::as_str).unwrap_or(fallback);
        Ok(ident(physical)?.to_string())
    };
    let eff = col("effectStartTime", "effect_start_time")?;
    let version = col("versionNo", "version_no")?;
    let created = col("createdTime", "created_time")?;
    let deleted = col("deleted", "is_deleted")?;

    let mut cond = format!("{eff} <= {}", time.as_of);
    if let Some(known) = &time.known_at {
        cond.push_str(&format!(" AND {created} <= {known}"));
    }
    let inner = if entity.write_once {
        format!("SELECT * FROM {table} WHERE {cond}")
    } else {
        format!(
            "SELECT DISTINCT ON ({pk}) * FROM {table} WHERE {cond} ORDER BY {pk}, {eff} DESC, {version} DESC"
        )
    };
    Ok(format!("(SELECT v.* FROM ({inner}) v WHERE NOT v.{deleted})"))
}

/// Names inside `{{...}}`, in order of appearance.
pub fn placeholder_names(body: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                names.push(after[..end].trim().to_string());
                rest = &after[end + 2..];
            }
            None => break,
        }
    }
    names
}

/// Replaces `{{Name}}`, `{{Entity.field}}` and `{{TemporalEntity}}` using the mapping.
/// A temporal entity becomes the subquery of the versions in effect at `time`.
/// Returns every unresolved name.
pub fn resolve_placeholders(body: &str, mapping: &Mapping, time: &TimeSql) -> Result<String, Vec<String>> {
    let mut out = String::with_capacity(body.len());
    let mut unresolved = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let name = after[..end].trim();
                let entity = name.split('.').next().unwrap_or(name);
                let replacement = match mapping.temporal.get(name) {
                    Some(t) => temporal_source(t, time).ok(),
                    None => mapping
                        .tables
                        .get(name)
                        .or_else(|| mapping.tables.get(entity))
                        .cloned(),
                };
                match replacement {
                    Some(text) => out.push_str(&text),
                    None => unresolved.push(name.to_string()),
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    if unresolved.is_empty() {
        Ok(out)
    } else {
        unresolved.sort();
        unresolved.dedup();
        Err(unresolved)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// SQL text, including string literals and comments (copied verbatim).
    Raw(String),
    /// A `:name` parameter.
    Param(String),
}

/// Splits SQL into raw text and `:name` parameters.
/// Skips `'...'` strings, `"..."` identifiers, `-- ...` and `/* ... */`
/// comments, and `::` casts.
pub fn tokenize(sql: &str) -> Vec<Token> {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut tokens = Vec::new();
    let mut raw = String::new();
    let mut i = 0;

    let flush = |raw: &mut String, tokens: &mut Vec<Token>| {
        if !raw.is_empty() {
            tokens.push(Token::Raw(std::mem::take(raw)));
        }
    };

    while i < n {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\'' | '"' => {
                // Copy a quoted string or identifier; doubled quotes stay inside it.
                raw.push(c);
                i += 1;
                while i < n {
                    raw.push(chars[i]);
                    if chars[i] == c {
                        if chars.get(i + 1) == Some(&c) {
                            raw.push(c);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            '-' if next == Some('-') => {
                while i < n && chars[i] != '\n' {
                    raw.push(chars[i]);
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                raw.push_str("/*");
                i += 2;
                while i < n {
                    if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                        raw.push_str("*/");
                        i += 2;
                        break;
                    }
                    raw.push(chars[i]);
                    i += 1;
                }
            }
            ':' => {
                let prev = if i > 0 { Some(chars[i - 1]) } else { None };
                let starts_ident = next.is_some_and(|ch| ch.is_alphabetic() || ch == '_');
                if starts_ident && prev != Some(':') {
                    flush(&mut raw, &mut tokens);
                    let mut name = String::new();
                    let mut j = i + 1;
                    while j < n && (chars[j].is_alphanumeric() || chars[j] == '_') {
                        name.push(chars[j]);
                        j += 1;
                    }
                    tokens.push(Token::Param(name));
                    i = j;
                } else {
                    // `::` cast: copy both colons so the second is not re-read as a param.
                    raw.push(c);
                    if next == Some(':') {
                        raw.push(':');
                        i += 1;
                    }
                    i += 1;
                }
            }
            _ => {
                raw.push(c);
                i += 1;
            }
        }
    }
    flush(&mut raw, &mut tokens);
    tokens
}

/// Names of all `:name` parameters used in the SQL, in order, without repeats.
pub fn used_params(sql: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    tokenize(sql)
        .into_iter()
        .filter_map(|t| match t {
            Token::Param(name) if seen.insert(name.clone()) => Some(name),
            _ => None,
        })
        .collect()
}

/// Quotes a value as an SQL literal. `None` becomes NULL.
/// Untyped literals take their type from context in PostgreSQL, so
/// `CAST('...' AS timestamptz)` and `id = '5'` both work without bind types.
pub fn sql_literal(value: Option<&str>) -> Result<String, String> {
    match value {
        None => Ok("NULL".to_string()),
        Some(v) => {
            if v.contains('\0') {
                return Err("parameter value contains a NUL byte".into());
            }
            Ok(format!("'{}'", v.replace('\'', "''")))
        }
    }
}

/// Renders the SQL with every `:name` replaced by its literal value.
pub fn render_sql(sql: &str, values: &BTreeMap<String, Option<String>>) -> Result<String, String> {
    let mut out = String::with_capacity(sql.len());
    for token in tokenize(sql) {
        match token {
            Token::Raw(text) => out.push_str(&text),
            Token::Param(name) => {
                let value = values
                    .get(&name)
                    .ok_or_else(|| format!("missing value for parameter :{name}"))?;
                out.push_str(&sql_literal(value.as_deref())?);
            }
        }
    }
    Ok(out)
}

/// Static checks that do not need a database. Returns human-readable problems.
pub fn lint(front: &FrontMatter, body: &str, mapping: &Mapping) -> Vec<String> {
    let mut problems = Vec::new();
    if let Err(names) = resolve_placeholders(body, mapping, &TimeSql::default()) {
        for name in names {
            problems.push(format!("placeholder {{{{{name}}}}} is not in the mapping"));
        }
    }
    let used = used_params(body);
    for name in &used {
        if !front.params.contains_key(name) {
            problems.push(format!("parameter :{name} is used in SQL but not declared"));
        }
    }
    for name in front.params.keys() {
        if !used.contains(name) {
            problems.push(format!("parameter {name} is declared but not used in SQL"));
        }
    }
    if let Some(spec) = &front.time_slice {
        for name in [&spec.as_of, &spec.known_at].into_iter().flatten() {
            if !front.params.contains_key(name) {
                problems.push(format!("timeSlice names parameter {name}, which is not declared"));
            }
        }
    }
    let mut temporal_used: Vec<String> = placeholder_names(body)
        .into_iter()
        .filter(|n| mapping.temporal.contains_key(n))
        .collect();
    temporal_used.sort();
    temporal_used.dedup();
    if !temporal_used.is_empty() && front.time_slice.is_none() {
        problems.push(format!(
            "reads temporal {}; the time is the As of field (empty = now)",
            temporal_used.join(", ")
        ));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"/*---
id: qb.sponsor.analytics.trend
description: test
params:
  publicationId: { like: QbPublicationCard.publicationId, description: "optional" }
  from: { kind: { type: temporal, role: EVENT_TIME }, required: true }
  bucket: { kind: { type: text, maxLength: 4 }, required: true }
---*/
-- :ignored in a comment
SELECT '{{QbStatsRun.zone}}' AS z, :from::text AS f
FROM {{QbStatsRun}} r
WHERE CAST(:publicationId AS uuid) IS NULL OR CAST(:from AS timestamptz) > now()
  AND 'it''s :notparam' <> :bucket;
"#;

    #[test]
    fn parses_front_matter_and_body() {
        let (front, body) = parse_source(SAMPLE).unwrap();
        assert_eq!(front.id, "qb.sponsor.analytics.trend");
        let names: Vec<_> = front.params.keys().cloned().collect();
        assert_eq!(names, vec!["publicationId", "from", "bucket"]);
        assert!(front.params["from"].required);
        assert_eq!(front.params["bucket"].kind.as_ref().unwrap().max_length, Some(4));
        assert!(body.starts_with("-- :ignored"));
    }

    #[test]
    fn finds_params_outside_literals_comments_and_casts() {
        assert_eq!(used_params(SAMPLE.split("---*/\n").nth(1).unwrap()), vec!["from", "publicationId", "bucket"]);
    }

    #[test]
    fn resolves_placeholders_by_field_then_entity() {
        let mut m = BTreeMap::new();
        m.insert("QbStatsRun".to_string(), "qb_stats_run".to_string());
        m.insert("QbStatsRun.zone".to_string(), "\"zone\"".to_string());
        let m = Mapping { tables: m, ..Default::default() };
        let t = TimeSql::default();
        let out = resolve_placeholders("{{QbStatsRun.zone}} {{QbStatsRun}}", &m, &t).unwrap();
        assert_eq!(out, "\"zone\" qb_stats_run");
        let err = resolve_placeholders("{{Missing.x}}", &m, &t).unwrap_err();
        assert_eq!(err, vec!["Missing.x"]);
    }

    #[test]
    fn renders_literals_and_nulls() {
        let mut values = BTreeMap::new();
        values.insert("publicationId".to_string(), None);
        values.insert("from".to_string(), Some("2026-01-01T00:00:00Z".to_string()));
        values.insert("bucket".to_string(), Some("HOUR".to_string()));
        let sql = "SELECT :from, CAST(:publicationId AS uuid), 'x :from', :bucket";
        let out = render_sql(sql, &values).unwrap();
        assert_eq!(
            out,
            "SELECT '2026-01-01T00:00:00Z', CAST(NULL AS uuid), 'x :from', 'HOUR'"
        );
    }

    #[test]
    fn escapes_single_quotes_in_values() {
        assert_eq!(sql_literal(Some("a'b")).unwrap(), "'a''b'");
    }

    #[test]
    fn lint_reports_undeclared_and_unused() {
        let (front, body) = parse_source(SAMPLE).unwrap();
        let m = Mapping::default(); // empty mapping: {{QbStatsRun.zone}} must be reported
        let problems = lint(&front, &body, &m);
        assert!(problems.iter().any(|p| p.contains("{{QbStatsRun.zone}}")));
        let (front2, _) = parse_source(SAMPLE).unwrap();
        let problems2 = lint(&front2, "SELECT :from", &m);
        assert!(problems2.iter().any(|p| p.contains("bucket is declared but not used")));
    }

    fn sys_param_mapping() -> Mapping {
        parse_mapping(
            r#"{
              "SysParam": {"table": "sys_param_version", "pk": "param_id", "writeOnce": false,
                "columns": {"versionNo": "version_no", "effectStartTime": "effect_start_time",
                            "createdTime": "created_time", "deleted": "is_deleted"}},
              "SysParam.value": "param_value"
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn temporal_entity_reads_the_latest_version_in_effect_and_skips_tombstones() {
        let m = sys_param_mapping();
        let time = TimeSql { as_of: "CAST('2026-01-15T00:00:00Z' AS timestamptz)".into(), known_at: None };
        let out = resolve_placeholders("FROM {{SysParam}} p WHERE {{SysParam.value}} = 1", &m, &time).unwrap();
        assert_eq!(
            out,
            "FROM (SELECT v.* FROM (SELECT DISTINCT ON (param_id) * FROM sys_param_version WHERE effect_start_time <= CAST('2026-01-15T00:00:00Z' AS timestamptz) ORDER BY param_id, effect_start_time DESC, version_no DESC) v WHERE NOT v.is_deleted) p WHERE param_value = 1"
        );
    }

    #[test]
    fn temporal_entity_adds_known_at_and_write_once_reads_without_distinct() {
        let mut m = sys_param_mapping();
        m.temporal.get_mut("SysParam").unwrap().write_once = true;
        let time = TimeSql { as_of: "now()".into(), known_at: Some("CAST('2026-02-01T00:00:00Z' AS timestamptz)".into()) };
        let out = resolve_placeholders("{{SysParam}}", &m, &time).unwrap();
        assert_eq!(
            out,
            "(SELECT v.* FROM (SELECT * FROM sys_param_version WHERE effect_start_time <= now() AND created_time <= CAST('2026-02-01T00:00:00Z' AS timestamptz)) v WHERE NOT v.is_deleted)"
        );
    }

    #[test]
    fn time_sql_uses_form_values_and_defaults_to_now() {
        let spec = Some(TimeSliceSpec { as_of: Some("asOf".into()), known_at: Some("knownAt".into()) });
        let mut values = BTreeMap::new();
        values.insert("asOf".to_string(), Some("2026-01-15T00:00:00Z".to_string()));
        values.insert("knownAt".to_string(), None);
        let t = time_sql(&spec, &values).unwrap();
        assert_eq!(t.as_of, "CAST('2026-01-15T00:00:00Z' AS timestamptz)");
        assert_eq!(t.known_at, None);
        // No timeSlice at all: now, no record-time limit.
        let t = time_sql(&None, &values).unwrap();
        assert_eq!(t.as_of, "now()");
    }

    #[test]
    fn lint_warns_about_temporal_reads_without_time_slice() {
        let (front, _) = parse_source(SAMPLE).unwrap();
        let m = sys_param_mapping();
        let problems = lint(&front, "SELECT 1 FROM {{SysParam}}", &m);
        assert!(problems.iter().any(|p| p.contains("reads temporal SysParam; the time is the As of field")));
    }

    #[test]
    fn run_time_takes_as_of_only_without_time_slice() {
        // Without timeSlice: the As of value is used; empty means now.
        let t = run_time(&None, &BTreeMap::new(), Some("2026-01-15T00:00:00Z")).unwrap();
        assert_eq!(t.as_of, "CAST('2026-01-15T00:00:00Z' AS timestamptz)");
        let t = run_time(&None, &BTreeMap::new(), Some("  ")).unwrap();
        assert_eq!(t.as_of, "now()");
        // With timeSlice: the As of value is ignored; the slice's parameters decide.
        let spec = Some(TimeSliceSpec { as_of: Some("asOf".into()), known_at: None });
        let mut values = BTreeMap::new();
        values.insert("asOf".to_string(), Some("2026-02-01T00:00:00Z".to_string()));
        let t = run_time(&spec, &values, Some("2026-01-15T00:00:00Z")).unwrap();
        assert_eq!(t.as_of, "CAST('2026-02-01T00:00:00Z' AS timestamptz)");
    }

    #[test]
    fn parse_mapping_accepts_strings_and_temporal_objects() {
        let m = sys_param_mapping();
        assert_eq!(m.tables["SysParam.value"], "param_value");
        assert_eq!(m.temporal["SysParam"].pk.as_deref(), Some("param_id"));
        assert!(parse_mapping("[1]").is_err());
    }
}

#[cfg(test)]
mod real_file_tests {
    use super::*;

    /// Runs only when the quizbuks repo is checked out next to this project.
    #[test]
    fn parses_real_catalog_file_if_present() {
        let path = "/Users/zhongpingxue/quizbuks/repo_quizbuks/backend/quizbuks/src/main/resources/queries/qb/sponsor/analytics-trend.sql";
        let Ok(src) = std::fs::read_to_string(path) else { return };
        let (front, body) = parse_source(&src).unwrap();
        assert_eq!(front.id, "qb.sponsor.analytics.trend");
        assert_eq!(
            front.params.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["publicationId", "from", "to", "bucket"]
        );
        assert_eq!(
            used_params(&body),
            vec!["from", "to", "bucket", "publicationId"]
        );
        let names = resolve_placeholders(&body, &Mapping::default(), &TimeSql::default()).unwrap_err();
        assert!(names.contains(&"QbStatsRun.zone".to_string()));
        assert!(names.contains(&"QbAnalyticsHour.hourStart".to_string()));
    }
}

