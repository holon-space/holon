//! The generic TOON primitive layer: scalar quoting/escaping and row
//! splitting, implemented to the TOON v4 draft spec (indentation-based,
//! comma-delimited, `\n`-escaped — TOON has no block-scalar form, so every
//! newline in a value is escaped).
//!
//! On top of the spec primitives sits a small **props sub-format** — a
//! `key=value` list packed into a single cell. TOON tabular rows are flat
//! (one scalar per leaf field), so the block's rare/heterogeneous fields
//! (priority, tags, source language, scheduling, `requires`, arbitrary drawer
//! keys) cannot each be a column without bloating every row; they are folded
//! into one `props` cell that is empty for the common case. The double
//! encoding (props escaping, then TOON scalar quoting) is the documented
//! "escaping cost" of the representation.

use crate::error::Result;
use crate::error::ToonError;

// ---------------------------------------------------------------------------
// Scalar quoting / escaping (TOON §7)
// ---------------------------------------------------------------------------

/// TOON's mandatory-quoting predicate (§7.2), with the active delimiter fixed
/// to comma (tabular rows). Returns true when `s` cannot be written bare.
pub fn must_quote(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    let first = s.chars().next().unwrap();
    let last = s.chars().next_back().unwrap();
    if matches!(first, ' ' | '\t') || matches!(last, ' ' | '\t') {
        return true;
    }
    if matches!(s, "true" | "false" | "null") {
        return true;
    }
    if is_numeric_like(s) {
        return true;
    }
    if s == "-" || s.starts_with('-') || s == "#" || s.starts_with('#') {
        return true;
    }
    s.chars()
        .any(|c| matches!(c, ':' | '"' | '\\' | '[' | ']' | '{' | '}' | ',') || c.is_control())
}

fn is_numeric_like(s: &str) -> bool {
    // ^[+-]?[0-9]+(\.[0-9]+)?([eE][+-]?[0-9]+)?$
    let mut chars = s.chars().peekable();
    if matches!(chars.peek(), Some('+') | Some('-')) {
        chars.next();
    }
    let mut saw_int = false;
    while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
        chars.next();
        saw_int = true;
    }
    if !saw_int {
        return false;
    }
    if chars.peek() == Some(&'.') {
        chars.next();
        let mut saw_frac = false;
        while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
            chars.next();
            saw_frac = true;
        }
        if !saw_frac {
            return false;
        }
    }
    if matches!(chars.peek(), Some('e') | Some('E')) {
        chars.next();
        if matches!(chars.peek(), Some('+') | Some('-')) {
            chars.next();
        }
        let mut saw_exp = false;
        while matches!(chars.peek(), Some(c) if c.is_ascii_digit()) {
            chars.next();
            saw_exp = true;
        }
        if !saw_exp {
            return false;
        }
    }
    chars.peek().is_none()
}

pub fn escape_inner(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Encode one scalar as a TOON row cell. Empty string encodes as a *bare empty
/// token* (nothing) rather than `""`: within this crate's fixed schema an empty
/// cell always means "absent/empty", so the disambiguation `""` would only cost
/// tokens. Every non-empty value that trips [`must_quote`] is quoted+escaped.
pub fn encode_cell(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if must_quote(s) {
        format!("\"{}\"", escape_inner(s))
    } else {
        s.to_string()
    }
}

/// Decode one already-split row token (surrounding U+0020 spaces trimmed) into
/// its string value. A bare empty token is the empty string; a `"..."` token is
/// unescaped per §7.1.
pub fn decode_cell(token: &str, row: usize, context: &str) -> Result<String> {
    let token = token.trim_matches(' ');
    if token.is_empty() {
        return Ok(String::new());
    }
    if !token.starts_with('"') {
        return Ok(token.to_string());
    }
    // Quoted: must end with an unescaped closing quote and contain nothing after.
    let inner = &token[1..];
    if !inner.ends_with('"') || inner.len() < 1 {
        return Err(ToonError::UnterminatedQuote {
            row,
            context: context.to_string(),
            token: token.to_string(),
        });
    }
    let body = &inner[..inner.len() - 1];
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                if hex.len() != 4 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err(ToonError::BadUnicodeEscape { row, got: hex });
                }
                let cp = u32::from_str_radix(&hex, 16).unwrap();
                match char::from_u32(cp) {
                    Some(ch) => out.push(ch),
                    None => {
                        return Err(ToonError::BadUnicodeEscape { row, got: hex });
                    }
                }
            }
            other => {
                return Err(ToonError::BadEscape {
                    row,
                    context: context.to_string(),
                    escape: format!("\\{}", other.map(|c| c.to_string()).unwrap_or_default()),
                });
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Row split / join
// ---------------------------------------------------------------------------

/// Join pre-encoded cells with the comma delimiter.
pub fn join_row(cells: &[String]) -> String {
    cells.join(",")
}

/// Split a row on top-level commas, honouring quoted cells (a comma inside a
/// `"..."` cell is literal). Returns the **raw** tokens with their quotes still
/// attached — the caller decides whether to [`decode_cell`] them (string cells)
/// or classify them by their bare/quoted shape (the generic typed layer, which
/// must tell a bare `null`/`42`/`true` from the quoted strings
/// `"null"`/`"42"`).
pub fn split_row_tokens(line: &str, row: usize) -> Result<Vec<String>> {
    let mut raw: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut escaped = false;
    for c in line.chars() {
        if in_quote {
            cur.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_quote = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_quote = true;
                cur.push(c);
            }
            ',' => {
                raw.push(std::mem::take(&mut cur));
            }
            _ => cur.push(c),
        }
    }
    if in_quote {
        return Err(ToonError::UnterminatedQuote {
            row,
            context: "row".to_string(),
            token: line.to_string(),
        });
    }
    raw.push(cur);
    Ok(raw)
}

/// Split a row on top-level commas, honouring quoted cells (a comma inside a
/// `"..."` cell is literal), then decode each cell. Enforces exactly
/// `expected` cells.
pub fn parse_row(line: &str, expected: usize, row: usize) -> Result<Vec<String>> {
    let raw = split_row_tokens(line, row)?;

    if raw.len() != expected {
        return Err(ToonError::CellCountMismatch {
            row,
            expected,
            found: raw.len(),
            line: line.to_string(),
        });
    }

    let mut cells = Vec::with_capacity(raw.len());
    for (i, tok) in raw.iter().enumerate() {
        cells.push(decode_cell(tok, row, &format!("cell {}", i))?);
    }
    Ok(cells)
}

// ---------------------------------------------------------------------------
// Props sub-format: a space-separated `key=value` list inside one cell.
// ---------------------------------------------------------------------------

/// Escape a props key or value: backslash, then the three structural chars of
/// the sub-format (space, `=`) and the list separator (`,`, used inside TAGS /
/// REQUIRES values).
pub fn escape_props(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ' ' => out.push_str("\\s"),
            '=' => out.push_str("\\e"),
            ',' => out.push_str("\\c"),
            c => out.push(c),
        }
    }
    out
}

fn unescape_props(s: &str, row: usize) -> Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('\\') => out.push('\\'),
            Some('s') => out.push(' '),
            Some('e') => out.push('='),
            Some('c') => out.push(','),
            other => {
                return Err(ToonError::BadEscape {
                    row,
                    context: "props".to_string(),
                    escape: format!("\\{}", other.map(|c| c.to_string()).unwrap_or_default()),
                });
            }
        }
    }
    Ok(out)
}

/// Encode a list of strings into one props *value* (comma-separated, with
/// element commas and backslashes escaped). Distinct from [`escape_props`]:
/// this runs *before* the props layer, so an element may contain any char —
/// the props layer then escapes the whole string opaquely. Without this,
/// a comma inside a tag/id would be mistaken for the list separator.
pub fn encode_list(items: &[String]) -> String {
    items
        .iter()
        .map(|s| s.replace('\\', "\\\\").replace(',', "\\,"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Inverse of [`encode_list`]. An empty input string decodes to a single empty
/// element (`[""]`), matching `encode_list(&["".into()])`.
pub fn decode_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => cur.push('\\'),
                Some(',') => cur.push(','),
                Some(other) => {
                    cur.push('\\');
                    cur.push(other);
                }
                None => cur.push('\\'),
            }
        } else if c == ',' {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

/// Build the props cell payload (before TOON scalar quoting) from ordered
/// `(key, value)` pairs. Returns the empty string when there are no pairs.
pub fn encode_props_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", escape_props(k), escape_props(v)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse the (already TOON-decoded) props cell back into ordered `(key, value)`
/// pairs. Splits on unescaped spaces, then each token on its first unescaped
/// `=`. Fails loud on a token with no `=`.
pub fn decode_props_pairs(cell: &str, row: usize) -> Result<Vec<(String, String)>> {
    if cell.is_empty() {
        return Ok(Vec::new());
    }
    let tokens = split_unescaped(cell, ' ');
    let mut pairs = Vec::with_capacity(tokens.len());
    for tok in tokens {
        if tok.is_empty() {
            continue;
        }
        let eq = find_unescaped(&tok, '=').ok_or_else(|| ToonError::BadPropsEntry {
            row,
            entry: tok.clone(),
        })?;
        let key = unescape_props(&tok[..eq], row)?;
        let val = unescape_props(&tok[eq + 1..], row)?;
        pairs.push((key, val));
    }
    Ok(pairs)
}

/// Split on an unescaped occurrence of `sep` (a `\`-prefixed `sep` is kept in
/// the token). Backslash itself is treated as an escape introducer.
fn split_unescaped(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut escaped = false;
    for c in s.chars() {
        if escaped {
            cur.push('\\');
            cur.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == sep {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    if escaped {
        cur.push('\\');
    }
    out.push(cur);
    out
}

/// Byte index of the first unescaped `target`, or `None`.
fn find_unescaped(s: &str, target: char) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == target {
            return Some(i);
        }
    }
    None
}
