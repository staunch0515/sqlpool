//! CSV as psql writes it and as the app exports it. An unquoted empty field is NULL; `""` is an empty
//! string. Fields with a comma, a quote or a line break are quoted, with quotes doubled.

/// Parses CSV text into records of optional fields (`None` is NULL).
pub fn parse(text: &str) -> Vec<Vec<Option<String>>> {
    let mut records = Vec::new();
    let mut record: Vec<Option<String>> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    let mut pending = false;

    fn end_field(record: &mut Vec<Option<String>>, field: &mut String, quoted: &mut bool) {
        let value = if !*quoted && field.is_empty() {
            None
        } else {
            Some(std::mem::take(field))
        };
        field.clear();
        record.push(value);
        *quoted = false;
    }

    while let Some(c) = chars.next() {
        pending = true;
        if in_quotes {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() && !quoted => {
                in_quotes = true;
                quoted = true;
            }
            ',' => end_field(&mut record, &mut field, &mut quoted),
            '\r' => {}
            '\n' => {
                end_field(&mut record, &mut field, &mut quoted);
                records.push(std::mem::take(&mut record));
                pending = false;
            }
            other => field.push(other),
        }
    }
    if pending {
        end_field(&mut record, &mut field, &mut quoted);
        records.push(record);
    }
    records
}

fn field(out: &mut String, value: &Option<String>) {
    match value {
        None => {}
        Some(v) if v.is_empty() => out.push_str("\"\""),
        Some(v) => {
            if v.contains([',', '"', '\n', '\r']) {
                out.push('"');
                out.push_str(&v.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(v);
            }
        }
    }
}

/// Writes a header line and the rows, with CRLF line ends (what spreadsheets expect).
pub fn write(columns: &[String], rows: &[Vec<Option<String>>]) -> String {
    let mut out = String::new();
    let header: Vec<Option<String>> = columns.iter().map(|c| Some(c.clone())).collect();
    for (i, v) in header.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        field(&mut out, v);
    }
    out.push_str("\r\n");
    for row in rows {
        for (i, v) in row.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            field(&mut out, v);
        }
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_nulls_quotes_and_newlines() {
        let rows = parse("a,b,c\n1,,\"\"\n\"x,\"\"y\"\"\",\"line1\nline2\",3\n");
        assert_eq!(rows[0], vec![Some("a".into()), Some("b".into()), Some("c".into())]);
        assert_eq!(rows[1], vec![Some("1".into()), None, Some(String::new())]);
        assert_eq!(
            rows[2],
            vec![Some("x,\"y\"".into()), Some("line1\nline2".into()), Some("3".into())]
        );
        assert_eq!(parse("").len(), 0);
        assert_eq!(parse("only\n").len(), 1);
    }

    #[test]
    fn write_then_parse_gives_the_same_values() {
        let columns = vec!["id".to_string(), "note".to_string(), "empty".to_string()];
        let rows = vec![
            vec![Some("1".into()), Some("a,\"b\"\nc".into()), None],
            vec![Some("2".into()), None, Some(String::new())],
        ];
        let text = write(&columns, &rows);
        let back = parse(&text);
        assert_eq!(back[0], columns.iter().map(|c| Some(c.clone())).collect::<Vec<_>>());
        assert_eq!(back[1..], rows[..]);
    }
}
