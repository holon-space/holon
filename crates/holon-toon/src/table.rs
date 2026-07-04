//! Generic tabular TOON codec — the block-independent core, exposed for reuse
//! (e.g. compressing `holon` MCP query results, which are uniform JSON rows
//! that pay the repeated-keys tax TOON was built to remove).
//!
//! This layer sits directly on the shared scalar quoting/escaping primitives in
//! [`crate::toon`] (`must_quote`, `escape_inner`, `decode_cell`,
//! `split_row_tokens`) — there is exactly ONE escaping implementation in the
//! crate; the block schema (`schema.rs` / `renderer.rs` / `parser.rs`) and this
//! generic table are two policies over it.
//!
//! # Shape
//!
//! A [`Table`] renders to a single TOON tabular array:
//!
//! ```text
//! rows[3]{id,name,done}:
//!   1,alice,true
//!   2,bob,false
//!   3,"carol, jr.",
//! ```
//!
//! - **Columns** are the **lexicographically-sorted union** of every key that
//!   appears in any row (see [`Table::from_rows`]). Sorted (not first-seen) so
//!   the output is deterministic regardless of row/map iteration order — the
//!   input rows are unordered maps, so there is no meaningful "first" key to
//!   preserve.
//! - A **missing key** renders as an **empty cell** and parses back as
//!   **absent** (the key is simply not in the row's map) — distinct from an
//!   **empty string**, which renders as the quoted token `""`. This
//!   absent-vs-empty distinction is the one place a naive tabular format loses
//!   information, so it is encoded explicitly here.
//!
//! # Value model & round-trip
//!
//! [`ToonValue`] covers `Str` / `Int` / `Float` / `Bool` / `Null`. The codec is
//! a genuine bijection at the cell level: every cell decodes to exactly the
//! `ToonValue` that produced it, because the encoder quotes any string that
//! would otherwise be mistaken for a bare literal:
//!
//! | `ToonValue`      | cell token   | round-trips to |
//! |------------------|--------------|----------------|
//! | *(key absent)*   | *(empty)*    | *(key absent)* |
//! | `Str("")`        | `""`         | `Str("")`      |
//! | `Str("hi")`      | `hi`         | `Str("hi")`    |
//! | `Str("42")`      | `"42"`       | `Str("42")`    |
//! | `Str("null")`    | `"null"`     | `Str("null")`  |
//! | `Int(42)`        | `42`         | `Int(42)`      |
//! | `Float(1.5)`     | `1.5`        | `Float(1.5)`   |
//! | `Float(1.0)`     | `1.0`        | `Float(1.0)`   |
//! | `Bool(true)`     | `true`       | `Bool(true)`   |
//! | `Null`           | `null`       | `Null`         |
//!
//! # Nested JSON (SQL/JSON columns): JSON-string-in-cell
//!
//! A SQL row can carry a JSON object/array column. This codec is flat — one
//! scalar per cell — so nested values are **not** flattened into extra columns;
//! the caller serializes them to a JSON string and stores that as a
//! [`ToonValue::Str`]. That is lossless at the byte level (the cell quoting
//! already handles arbitrary strings) and simple. The documented limitation:
//! the *value type* "this was JSON, not a string that happens to look like
//! JSON" is not recovered on parse — the agent reads JSON text in the cell. See
//! [`ToonValue::from_json`], which performs this conversion and fails loud on
//! the only unrepresentable scalar (a non-finite JSON number, which JSON itself
//! forbids).

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use crate::error::Result;
use crate::error::ToonError;
use crate::toon::decode_cell;
use crate::toon::escape_inner;
use crate::toon::must_quote;
use crate::toon::split_row_tokens;

/// One tabular scalar. `Str` is an arbitrary string (including the empty
/// string); the numeric/bool/null variants are the bare TOON literals.
#[derive(Clone, Debug, PartialEq)]
pub enum ToonValue {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

impl ToonValue {
    /// Convert a `serde_json::Value` into a `ToonValue`, folding nested
    /// objects/arrays into a JSON string cell (JSON-string-in-cell, see module
    /// docs). Fails loud only on a non-finite JSON number, which is not valid
    /// JSON in the first place.
    #[cfg(feature = "serde-json")]
    pub fn from_json(v: &serde_json::Value) -> Result<Self> {
        use serde_json::Value as J;
        Ok(match v {
            J::Null => ToonValue::Null,
            J::Bool(b) => ToonValue::Bool(*b),
            J::String(s) => ToonValue::Str(s.clone()),
            J::Number(n) => {
                if let Some(i) = n.as_i64() {
                    ToonValue::Int(i)
                } else {
                    let f = n.as_f64().ok_or_else(|| ToonError::NonFiniteFloat {
                        value: n.to_string(),
                    })?;
                    if !f.is_finite() {
                        return Err(ToonError::NonFiniteFloat {
                            value: format!("{f:?}"),
                        });
                    }
                    ToonValue::Float(f)
                }
            }
            // Nested: serialize to compact JSON and carry it as a string cell.
            J::Array(_) | J::Object(_) => ToonValue::Str(
                serde_json::to_string(v).expect("serde_json::Value always re-serializes"),
            ),
        })
    }
}

/// A single row: keys present in the map are present cells, keys absent from
/// the map are absent cells (which are NOT the same as `Str("")`).
pub type Row = BTreeMap<String, ToonValue>;

/// A generic TOON tabular array: a name (the array key), an explicit ordered
/// column set, and the rows.
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

impl Table {
    /// Build a table from rows, deriving `columns` as the lexicographically
    /// sorted union of all keys. Fails loud if `name` is not a representable
    /// array key.
    pub fn from_rows(name: impl Into<String>, rows: Vec<Row>) -> Result<Self> {
        let name = name.into();
        check_table_name(&name)?;
        let mut cols: BTreeSet<String> = BTreeSet::new();
        for row in &rows {
            for k in row.keys() {
                cols.insert(k.clone());
            }
        }
        Ok(Table {
            name,
            columns: cols.into_iter().collect(),
            rows,
        })
    }

    /// Render the table as a TOON document (header + indented rows, trailing
    /// newline). Fails loud on a non-finite float cell.
    pub fn render(&self) -> Result<String> {
        let mut out = String::new();
        out.push_str(&self.header_line());
        out.push('\n');
        for row in &self.rows {
            out.push_str(ROW_INDENT);
            out.push_str(&self.render_row(row)?);
            out.push('\n');
        }
        Ok(out)
    }

    fn header_line(&self) -> String {
        let cols = self
            .columns
            .iter()
            .map(|c| encode_field(c))
            .collect::<Vec<_>>()
            .join(",");
        format!("{}[{}]{{{}}}:", self.name, self.rows.len(), cols)
    }

    fn render_row(&self, row: &Row) -> Result<String> {
        // Zero-column tables have an empty row line (no cells at all).
        if self.columns.is_empty() {
            return Ok(String::new());
        }
        let mut cells = Vec::with_capacity(self.columns.len());
        for col in &self.columns {
            match row.get(col) {
                None => cells.push(String::new()), // absent → empty cell
                Some(v) => cells.push(encode_value(v)?),
            }
        }
        Ok(cells.join(","))
    }

    /// Parse a TOON document produced by [`Table::render`] back into a table.
    pub fn parse(input: &str) -> Result<Table> {
        let mut lines = input.lines();
        let header = loop {
            match lines.next() {
                Some(l) if l.trim().is_empty() => continue,
                Some(l) => break l,
                None => return Err(ToonError::EmptyDocument),
            }
        };
        let (name, columns, declared) = parse_table_header(header)?;

        // Every post-header line is a data row (indent stripped). We do NOT
        // filter blank lines: a zero-column table's rows are legitimately blank
        // (`ROW_INDENT` + nothing), so dropping blanks would lose them.
        // `str::lines` yields no trailing empty for the final newline, so a
        // well-formed document has exactly `declared` row lines.
        let row_lines: Vec<&str> = lines
            .map(|l| l.strip_prefix(ROW_INDENT).unwrap_or(l))
            .collect();

        if row_lines.len() != declared {
            return Err(ToonError::RowCountMismatch {
                declared,
                actual: row_lines.len(),
            });
        }

        let mut rows = Vec::with_capacity(row_lines.len());
        for (i, line) in row_lines.iter().enumerate() {
            rows.push(parse_data_row(line, &columns, i)?);
        }

        Ok(Table {
            name,
            columns,
            rows,
        })
    }
}

const ROW_INDENT: &str = "  ";

/// Reject a table name that would collide with the TOON header grammar.
fn check_table_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.chars().any(|c| {
            c.is_whitespace()
                || matches!(c, '[' | ']' | '{' | '}' | ',' | ':' | '"' | '\\')
                || c.is_control()
        })
    {
        return Err(ToonError::BadTableName {
            name: name.to_string(),
        });
    }
    Ok(())
}

/// Encode a header field (column name). Like a string cell, but the empty
/// string is written as `""` (a bare empty header field would be invisible).
fn encode_field(s: &str) -> String {
    encode_str(s)
}

/// Encode an arbitrary string as a cell token. Identical to
/// [`crate::toon::encode_cell`] EXCEPT the empty string maps to the explicit
/// token `""` rather than a bare empty token — in the generic codec an empty
/// cell already means "absent", so an empty *string* must be distinguishable.
fn encode_str(s: &str) -> String {
    if s.is_empty() {
        return "\"\"".to_string();
    }
    // Also quote anything the decoder's numeric classifier would grab as a bare
    // number (`is_bare_number` is looser than `must_quote`'s strict numeric
    // rule, so e.g. "1.2.3" / "1e" / "1-2" would render bare and then fail to
    // parse). Quoting keeps bare numeric tokens exclusive to Int/Float cells.
    if must_quote(s) || is_bare_number(s) {
        format!("\"{}\"", escape_inner(s))
    } else {
        s.to_string()
    }
}

/// Encode one typed value as a bare/quoted cell token.
fn encode_value(v: &ToonValue) -> Result<String> {
    Ok(match v {
        ToonValue::Str(s) => encode_str(s),
        ToonValue::Int(i) => i.to_string(),
        ToonValue::Float(f) => {
            if !f.is_finite() {
                return Err(ToonError::NonFiniteFloat {
                    value: format!("{f:?}"),
                });
            }
            // `{:?}` guarantees a decimal point (`1.0`, not `1`) so the token
            // decodes back to Float, never Int.
            format!("{:?}", f)
        }
        ToonValue::Bool(b) => b.to_string(),
        ToonValue::Null => "null".to_string(),
    })
}

/// Parse the header `name[N]{c1,c2,...}:` → (name, columns, declared row
/// count).
fn parse_table_header(header: &str) -> Result<(String, Vec<String>, usize)> {
    let bad = || ToonError::BadTableHeader {
        got: header.to_string(),
    };
    // name up to the first '[' (names never contain '[', per check_table_name).
    let lb = header.find('[').ok_or_else(bad)?;
    let name = header[..lb].to_string();
    check_table_name(&name).map_err(|_| bad())?;

    let rest = &header[lb + 1..];
    let rb = rest.find(']').ok_or_else(bad)?;
    let declared: usize = rest[..rb].parse().map_err(|_| bad())?;

    let rest = &rest[rb + 1..];
    let inner = rest
        .strip_prefix('{')
        .and_then(|r| r.strip_suffix("}:"))
        .ok_or_else(bad)?;

    let columns = if inner.is_empty() {
        Vec::new()
    } else {
        // Column names are comma-separated, honouring quotes, then decoded.
        let raw = split_row_tokens(inner, 0)?;
        let mut cols = Vec::with_capacity(raw.len());
        for tok in &raw {
            cols.push(decode_cell(tok, 0, "column header")?);
        }
        cols
    };

    Ok((name, columns, declared))
}

/// Parse one data row against the known columns into a [`Row`]. Empty cells are
/// absent keys; every present cell is classified into its [`ToonValue`].
fn parse_data_row(line: &str, columns: &[String], row: usize) -> Result<Row> {
    // Zero-column table: the row line must be empty and yields an empty map.
    if columns.is_empty() {
        if !line.is_empty() {
            return Err(ToonError::CellCountMismatch {
                row,
                expected: 0,
                found: 1,
                line: line.to_string(),
            });
        }
        return Ok(BTreeMap::new());
    }

    let tokens = split_row_tokens(line, row)?;
    if tokens.len() != columns.len() {
        return Err(ToonError::CellCountMismatch {
            row,
            expected: columns.len(),
            found: tokens.len(),
            line: line.to_string(),
        });
    }

    let mut out: Row = BTreeMap::new();
    for (col, tok) in columns.iter().zip(tokens.iter()) {
        if let Some(v) = decode_value(tok, row, col)? {
            out.insert(col.clone(), v);
        }
    }
    Ok(out)
}

/// Classify one raw row token into a typed value, or `None` for an absent
/// (empty) cell. A quoted token is always a `Str`; a bare token is a literal
/// (`null` / `true` / `false` / number) or, failing all of those, a bare `Str`.
fn decode_value(token: &str, row: usize, col: &str) -> Result<Option<ToonValue>> {
    let trimmed = token.trim_matches(' ');
    if trimmed.is_empty() {
        return Ok(None); // absent cell
    }
    if trimmed.starts_with('"') {
        return Ok(Some(ToonValue::Str(decode_cell(
            trimmed,
            row,
            &format!("column {col:?}"),
        )?)));
    }
    Ok(Some(match trimmed {
        "null" => ToonValue::Null,
        "true" => ToonValue::Bool(true),
        "false" => ToonValue::Bool(false),
        _ if is_bare_number(trimmed) => decode_number(trimmed, row)?,
        // A bare token that needed no quoting: a plain string.
        _ => ToonValue::Str(decode_cell(trimmed, row, &format!("column {col:?}"))?),
    }))
}

/// A bare token that looks like a JSON-ish number (matches [`must_quote`]'s
/// numeric rule, i.e. what the encoder would have emitted bare for an
/// Int/Float).
fn is_bare_number(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    if matches!(chars.peek(), Some('+') | Some('-')) {
        chars.next();
    }
    let mut saw = false;
    for c in chars {
        if c.is_ascii_digit() {
            saw = true;
        } else if !matches!(c, '.' | 'e' | 'E' | '+' | '-') {
            return false;
        }
    }
    saw
}

fn decode_number(s: &str, row: usize) -> Result<ToonValue> {
    // Integer iff no fractional/exponent marker.
    if !s.contains(['.', 'e', 'E']) {
        return s
            .parse::<i64>()
            .map(ToonValue::Int)
            .map_err(|_| ToonError::BadNumber {
                row,
                token: s.to_string(),
            });
    }
    let f: f64 = s.parse().map_err(|_| ToonError::BadNumber {
        row,
        token: s.to_string(),
    })?;
    if !f.is_finite() {
        return Err(ToonError::BadNumber {
            row,
            token: s.to_string(),
        });
    }
    Ok(ToonValue::Float(f))
}
