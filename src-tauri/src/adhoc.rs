//! Checks for the ad-hoc SQL that an AI client sends to the `run_sql` tool.
//!
//! This is a guard in front of the read-only transaction (`db::readonly_script`), not a replacement for it.
//! It accepts one SELECT (or WITH ... SELECT) and refuses anything else, refuses names that are not mapped
//! tables (or CTE names of the same statement), and refuses database internals. It is a lexical check, so it
//! can refuse valid SQL, but it is meant to fail closed. The database account should be read-only as well.

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    /// A keyword or identifier, lower-cased. Quoted identifiers are included without their quotes.
    Word(String),
    /// One of `( ) , ; .` and `:`.
    Punct(char),
    /// Numbers, operators and string literals.
    Other,
}

/// Words that may never appear in the statement (they write, change state or read outside the mapped tables).
const FORBIDDEN: &[&str] = &[
    "insert", "update", "delete", "merge", "drop", "alter", "create", "truncate", "grant", "revoke", "copy",
    "call", "do", "into", "vacuum", "analyze", "reindex", "cluster", "listen", "notify", "lock", "set", "reset",
    "begin", "commit", "rollback", "savepoint", "release", "prepare", "execute", "deallocate", "discard",
    "refresh", "comment", "lo_import", "lo_export", "dblink", "security",
];

/// Words after which a `(` is a list or a subquery, not a function call.
const NOT_A_FUNCTION_BEFORE_PAREN: &[&str] = &[
    "in", "from", "join", "as", "on", "and", "or", "not", "where", "exists", "any", "all", "select", "with",
    "using", "values", "having", "by", "when", "then", "else", "union", "intersect", "except",
];

/// Lexical tokens: comments and string literals are skipped (a string is `Other`).
fn scan(sql: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = sql.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && chars.get(i + 1) == Some(&'-') {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && chars.get(i + 1) == Some(&'*') {
            i += 2;
            while i < n && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if c == '\'' {
            i += 1;
            loop {
                match chars.get(i) {
                    None => return Err("unterminated string literal".into()),
                    Some('\\') => return Err("backslashes are not allowed in run_sql".into()),
                    Some('\'') if chars.get(i + 1) == Some(&'\'') => i += 2,
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(_) => i += 1,
                }
            }
            out.push(Tok::Other);
        } else if c == '"' {
            let start = i + 1;
            i = start;
            while i < n && chars[i] != '"' {
                i += 1;
            }
            if i >= n {
                return Err("unterminated quoted name".into());
            }
            out.push(Tok::Word(chars[start..i].iter().collect::<String>().to_lowercase()));
            i += 1;
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Word(chars[start..i].iter().collect::<String>().to_lowercase()));
        } else if c.is_ascii_digit() {
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '.' || chars[i] == '_') {
                i += 1;
            }
            out.push(Tok::Other);
        } else if c == '\\' || c == '$' {
            // Backslash escapes and dollar quoting can hide text from this check, so they are refused.
            return Err(format!("the character {c} is not allowed in run_sql"));
        } else {
            out.push(if "(),;.:".contains(c) { Tok::Punct(c) } else { Tok::Other });
            i += 1;
        }
    }
    Ok(out)
}

/// Checks one statement. `allowed` holds the lower-cased physical names of the mapped tables.
pub fn check(sql: &str, allowed: &HashSet<String>) -> Result<(), String> {
    let mut toks = scan(sql)?;
    while toks.last() == Some(&Tok::Punct(';')) {
        toks.pop();
    }
    if toks.contains(&Tok::Punct(';')) {
        return Err("run_sql takes one statement".into());
    }
    match toks.first() {
        Some(Tok::Word(w)) if w == "select" || w == "with" => {}
        _ => return Err("run_sql takes a SELECT (or WITH ... SELECT) statement".into()),
    }

    // Names this statement defines itself (WITH name AS (...)) may be read.
    let mut ctes: HashSet<String> = HashSet::new();
    for w in toks.windows(3) {
        if let (Tok::Word(name), Tok::Word(kw), Tok::Punct('(')) = (&w[0], &w[1], &w[2])
            && kw == "as"
        {
            ctes.insert(name.clone());
        }
    }

    for t in &toks {
        if let Tok::Word(w) = t {
            if FORBIDDEN.contains(&w.as_str()) {
                return Err(format!("the word {w} is not allowed in run_sql"));
            }
            if w.starts_with("pg_") || w == "information_schema" {
                return Err(format!("{w} is a database internal and is not allowed in run_sql"));
            }
        }
    }

    // Table references: after FROM or JOIN, and after commas in a FROM list. A FROM inside a function call
    // (EXTRACT(epoch FROM ts)) is not a table reference.
    let mut parens: Vec<bool> = Vec::new(); // true: the parens of a function call
    let mut from_open: Vec<usize> = Vec::new(); // depths of FROM lists that are still open
    let mut expect_ref = false;
    let mut prev: Option<Tok> = None;
    let mut i = 0;
    while i < toks.len() {
        let depth = parens.len();
        let tok = toks[i].clone();
        match &tok {
            Tok::Punct('(') => {
                let is_fn = match &prev {
                    Some(Tok::Word(w)) => !NOT_A_FUNCTION_BEFORE_PAREN.contains(&w.as_str()),
                    _ => false,
                };
                parens.push(is_fn);
                expect_ref = false;
            }
            Tok::Punct(')') => {
                parens.pop();
                let len = parens.len();
                from_open.retain(|&d| d <= len);
            }
            Tok::Punct(',') => {
                if from_open.last() == Some(&depth) {
                    expect_ref = true;
                }
            }
            Tok::Word(w) if expect_ref => {
                if w == "lateral" || w == "only" {
                    prev = Some(tok);
                    i += 1;
                    continue;
                }
                let mut name = w.clone();
                let mut j = i + 1;
                while let (Some(Tok::Punct('.')), Some(Tok::Word(next))) = (toks.get(j), toks.get(j + 1)) {
                    name.push('.');
                    name.push_str(next);
                    j += 2;
                }
                let is_call = matches!(toks.get(j), Some(Tok::Punct('(')));
                if !is_call && !ctes.contains(&name) && !allowed.contains(&name) {
                    return Err(format!("{name} is not one of the mapped tables; list_tables shows them"));
                }
                expect_ref = false;
                prev = toks.get(j - 1).cloned();
                i = j;
                continue;
            }
            Tok::Word(w) => match w.as_str() {
                "from" => {
                    let in_function = parens.last() == Some(&true);
                    let is_distinct_from = prev == Some(Tok::Word("distinct".into()));
                    if !in_function && !is_distinct_from {
                        from_open.push(depth);
                        expect_ref = true;
                    }
                }
                "join" => expect_ref = true,
                "where" | "group" | "order" | "having" | "limit" | "offset" | "union" | "intersect" | "except"
                | "window" | "fetch" | "returning" => from_open.retain(|&d| d != depth),
                _ => {}
            },
            _ => expect_ref = false,
        }
        prev = Some(tok);
        i += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> HashSet<String> {
        ["orders", "customers"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn accepts_plain_selects_over_mapped_tables() {
        let ok = [
            "SELECT count(*) FROM orders o JOIN customers c ON c.id = o.customer_id WHERE o.total > 0;",
            "WITH x AS (SELECT id FROM orders) SELECT * FROM x",
            "SELECT extract(epoch from created_at) FROM orders",
            "SELECT 'drop table secret', \"orders\" FROM orders -- delete from secret",
            "SELECT * FROM orders WHERE id IN (SELECT id FROM customers)",
            "SELECT o.id FROM orders o, customers c WHERE c.id = o.customer_id",
            "SELECT * FROM orders WHERE (id, total) IN ((1, 2), (3, 4))",
            "SELECT 1",
        ];
        for sql in ok {
            assert!(check(sql, &allowed()).is_ok(), "should pass: {sql}");
        }
    }

    #[test]
    fn refuses_unmapped_tables_including_after_commas_and_joins() {
        let bad = [
            "SELECT * FROM secret_table",
            "SELECT * FROM orders, secret_table",
            "SELECT * FROM orders JOIN secret_table ON true",
            "SELECT * FROM (SELECT * FROM secret_table) s",
            "SELECT * FROM public.orders",
            "SELECT * FROM information_schema.tables",
        ];
        for sql in bad {
            assert!(check(sql, &allowed()).is_err(), "should refuse: {sql}");
        }
    }

    #[test]
    fn refuses_writes_and_multiple_statements() {
        let bad = [
            "DELETE FROM orders",
            "WITH d AS (DELETE FROM orders RETURNING *) SELECT 1",
            "SELECT 1; DROP TABLE orders",
            "SELECT 1 INTO x",
            "SELECT pg_sleep(1)",
            "SELECT * FROM orders FOR UPDATE; SELECT 2",
            "",
            "SHOW ALL",
        ];
        for sql in bad {
            assert!(check(sql, &allowed()).is_err(), "should refuse: {sql}");
        }
    }

    #[test]
    fn refuses_escapes_and_dollar_quoting() {
        assert!(check("SELECT '\\'", &allowed()).is_err());
        assert!(check("SELECT $$x$$", &allowed()).is_err());
    }
}
