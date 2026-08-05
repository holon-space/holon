use holon_api::Value;
use serde_json;

/// Convert a `Value` to a SQL literal string suitable for embedding in raw SQL.
///
/// Array and Object types are serialized as JSON strings.
pub fn value_to_sql_literal(value: &Value) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Boolean(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) | Value::DateTime(s) | Value::Json(s) => {
            format!("'{}'", s.replace('\'', "''"))
        }
        Value::Array(_) | Value::Object(_) => {
            let json: serde_json::Value = value.clone().into();
            let s = serde_json::to_string(&json).expect("Value→JSON serialization cannot fail");
            format!("'{}'", s.replace('\'', "''"))
        }
    }
}

/// The sigils SQLite accepts in front of a named parameter. All three are
/// equally real placeholders: a scanner that knows only `$name` leaves `:name`
/// in the statement, binds nothing, and SQLite reads the unbound placeholder as
/// NULL — the query then matches nothing and returns success.
const NAMED_PARAM_SIGILS: [char; 3] = ['$', ':', '@'];

fn is_param_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Rewrite every named parameter placeholder (`$name`, `:name`, `@name`) that
/// occurs in SQL *code*, leaving string literals, quoted identifiers and
/// comments byte-identical.
///
/// Skipping quoted spans is not a nicety: schemed entity ids put a colon in the
/// middle of nearly every literal we query with (`WHERE parent_id =
/// 'block:1820f890-…'`), and treating that colon as a placeholder would turn a
/// working literal query into a binding error.
///
/// `substitute` receives the placeholder's name (without sigil) and returns the
/// text to emit in its place, or `None` to emit the placeholder verbatim. The
/// scanner holds no policy about missing values — a caller that must not lose a
/// binding records the misses its closure saw and fails on them.
pub fn rewrite_named_params(
    sql: &str,
    substitute: &mut dyn FnMut(&str) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            // Quoted spans: SQL string literals, and the three identifier
            // quotings SQLite accepts. Doubling the quote escapes it, which
            // copy_through handles by simply continuing past the pair.
            '\'' | '"' | '`' => {
                out.push(ch);
                copy_through(&mut chars, &mut out, ch);
            }
            '[' => {
                out.push(ch);
                copy_through(&mut chars, &mut out, ']');
            }
            '-' if chars.peek() == Some(&'-') => {
                out.push(ch);
                copy_through(&mut chars, &mut out, '\n');
            }
            '/' if chars.peek() == Some(&'*') => {
                out.push(ch);
                out.push(chars.next().expect("peeked '*'"));
                while let Some(c) = chars.next() {
                    out.push(c);
                    if c == '*' && chars.peek() == Some(&'/') {
                        out.push(chars.next().expect("peeked '/'"));
                        break;
                    }
                }
            }
            sigil if NAMED_PARAM_SIGILS.contains(&sigil) => {
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if is_param_name_char(c) {
                        name.push(chars.next().expect("peeked a name char"));
                    } else {
                        break;
                    }
                }
                // A lone sigil (`cost $ 5`) is ordinary text, not a placeholder.
                match if name.is_empty() {
                    None
                } else {
                    substitute(&name)
                } {
                    Some(replacement) => out.push_str(&replacement),
                    None => {
                        out.push(sigil);
                        out.push_str(&name);
                    }
                }
            }
            _ => out.push(ch),
        }
    }

    out
}

/// Copy characters into `out` up to and including the next `terminator`. An
/// unterminated span runs to the end of the input — the SQL is malformed and
/// the database is the right place to say so.
fn copy_through(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    out: &mut String,
    terminator: char,
) {
    for c in chars.by_ref() {
        out.push(c);
        if c == terminator {
            return;
        }
    }
}

/// Split a semicolon-delimited SQL file into individual statements.
///
/// Skips `;` inside `--` line comments — otherwise comments like
/// `-- closed (omitted); retained for back/forward` truncate the
/// surrounding CREATE TABLE statement and the parser sees "incomplete input".
pub fn sql_statements(content: &str) -> impl Iterator<Item = &str> {
    let bytes = content.as_bytes();
    let mut splits: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let mut in_line_comment = false;
    let mut prev_dash = false;
    for (i, &b) in bytes.iter().enumerate() {
        if in_line_comment {
            if b == b'\n' {
                in_line_comment = false;
            }
            prev_dash = false;
            continue;
        }
        if b == b'-' && prev_dash {
            in_line_comment = true;
            prev_dash = false;
            continue;
        }
        prev_dash = b == b'-';
        if b == b';' {
            splits.push((start, i));
            start = i + 1;
        }
    }
    splits.push((start, bytes.len()));
    splits
        .into_iter()
        .map(move |(a, b)| content[a..b].trim())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_escapes_quotes() {
        assert_eq!(
            value_to_sql_literal(&Value::String("it's".into())),
            "'it''s'"
        );
    }

    #[test]
    fn test_null() {
        assert_eq!(value_to_sql_literal(&Value::Null), "NULL");
    }

    #[test]
    fn test_boolean() {
        assert_eq!(value_to_sql_literal(&Value::Boolean(true)), "1");
        assert_eq!(value_to_sql_literal(&Value::Boolean(false)), "0");
    }

    #[test]
    fn test_integer() {
        assert_eq!(value_to_sql_literal(&Value::Integer(42)), "42");
    }

    #[test]
    fn test_float() {
        assert_eq!(value_to_sql_literal(&Value::Float(2.5)), "2.5");
    }

    #[test]
    fn test_array() {
        let val = Value::Array(vec![Value::Integer(1), Value::String("two".into())]);
        assert_eq!(value_to_sql_literal(&val), "'[1,\"two\"]'");
    }

    #[test]
    fn test_object() {
        use std::collections::HashMap;
        let val = Value::Object(HashMap::from([("key".into(), Value::String("val".into()))]));
        assert_eq!(value_to_sql_literal(&val), "'{\"key\":\"val\"}'");
    }

    // A `-` that is NOT part of a `--` comment must not swallow the following
    // `;`. Kills both `== b'-'`→`!= b'-'` and `&&`→`||` in the comment guard:
    // either mutation treats the arithmetic minus as a comment start and folds
    // the two statements into one.
    #[test]
    fn minus_operator_is_not_a_comment() {
        let stmts: Vec<&str> = sql_statements("SELECT a-1; SELECT 2").collect();
        assert_eq!(stmts, vec!["SELECT a-1", "SELECT 2"]);
    }

    // A `;` inside a `--` line comment must not split the statement — the
    // regression the function exists to prevent.
    #[test]
    fn semicolon_inside_line_comment_does_not_split() {
        let sql = "CREATE TABLE t (a INT -- note; keep\n);\nSELECT 1";
        let stmts: Vec<&str> = sql_statements(sql).collect();
        assert_eq!(stmts.len(), 2);
        assert!(stmts[0].contains("note; keep"));
        assert_eq!(stmts[1], "SELECT 1");
    }

    // ── rewrite_named_params ────────────────────────────────────────────────

    /// Collect the names the scanner offered, substituting `?` for every one it
    /// was given a value for.
    fn scan(sql: &str, known: &[&str]) -> (String, Vec<String>) {
        let mut seen = Vec::new();
        let out = rewrite_named_params(sql, &mut |name| {
            seen.push(name.to_string());
            known.contains(&name).then(|| "?".to_string())
        });
        (out, seen)
    }

    #[test]
    fn every_sqlite_named_param_sigil_is_a_placeholder() {
        for sigil in ['$', ':', '@'] {
            let (sql, seen) = scan(
                &format!("SELECT * FROM block WHERE id = {sigil}pid"),
                &["pid"],
            );
            assert_eq!(sql, "SELECT * FROM block WHERE id = ?", "sigil {sigil}");
            assert_eq!(seen, vec!["pid".to_string()], "sigil {sigil}");
        }
    }

    // The colon in a schemed id is the reason quoted spans must be skipped:
    // treating it as a placeholder breaks every literal id query.
    #[test]
    fn colon_inside_a_string_literal_is_not_a_placeholder() {
        let sql = "SELECT * FROM block WHERE parent_id = 'block:1820f890-aaaa'";
        let (out, seen) = scan(sql, &[]);
        assert_eq!(out, sql);
        assert!(
            seen.is_empty(),
            "offered names from inside a literal: {seen:?}"
        );
    }

    #[test]
    fn sigils_inside_quoted_identifiers_and_comments_are_inert() {
        for sql in [
            "SELECT \"a:b\" FROM t",
            "SELECT `a:b` FROM t",
            "SELECT [a:b] FROM t",
            "SELECT 1 -- see :pid\nFROM t",
            "SELECT /* :pid $pid @pid */ 1 FROM t",
            "SELECT json_extract(properties, '$.task_state') FROM block",
        ] {
            let (out, seen) = scan(sql, &[]);
            assert_eq!(out, sql, "rewrote {sql:?}");
            assert!(seen.is_empty(), "{sql:?} offered {seen:?}");
        }
    }

    #[test]
    fn doubled_quote_escape_keeps_the_scanner_inside_the_literal() {
        let sql = "SELECT * FROM t WHERE s = 'it''s :not a param' AND id = :pid";
        let (out, seen) = scan(sql, &["pid"]);
        assert_eq!(
            out,
            "SELECT * FROM t WHERE s = 'it''s :not a param' AND id = ?"
        );
        assert_eq!(seen, vec!["pid".to_string()]);
    }

    #[test]
    fn a_lone_sigil_is_ordinary_text() {
        for sql in ["cost $ 5", "a : b", "user @ host"] {
            let (out, seen) = scan(sql, &[]);
            assert_eq!(out, sql);
            assert!(seen.is_empty());
        }
    }

    #[test]
    fn an_unsubstituted_placeholder_is_emitted_verbatim() {
        let (out, seen) = scan("SELECT $a, :b, @c", &[]);
        assert_eq!(out, "SELECT $a, :b, @c");
        assert_eq!(
            seen,
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }

    #[test]
    fn underscores_and_digits_belong_to_the_name() {
        let (_, seen) = scan("SELECT :context_local_id, :p2", &[]);
        assert_eq!(seen, vec!["context_local_id".to_string(), "p2".to_string()]);
    }
}
