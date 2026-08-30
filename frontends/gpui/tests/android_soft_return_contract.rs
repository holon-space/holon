//! The source predicates that decide whether Android can start a new block.
//!
//! They live in the vendored IME host class, which no Rust test can execute: it
//! only runs inside the APK, against a real IME. These read it as source — with
//! comments stripped, and binding each decision to the branch that makes it, so
//! that reinstating the bug cannot leave them green.
//!
//! Entry: docs/Testing/bugfunnel/entries/2026-08-30-android-ime-no-enter-key.md

use std::path::PathBuf;

fn input_view_source() -> String {
    let path: PathBuf = [
        env!("CARGO_MANIFEST_DIR"),
        "android",
        "java",
        "dev",
        "gpui",
        "mobile",
        "GpuiTextInputView.java",
    ]
    .iter()
    .collect();
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read the vendored IME host class at {path:?}: {e}"));
    strip_comments(&source)
}

/// Java with `//` and `/* */` comments blanked out, string and char literals
/// left intact. Prose that merely names a constant must not satisfy a predicate
/// about the code.
fn strip_comments(source: &str) -> String {
    let bytes: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let next = bytes.get(i + 1).copied();
        match (c, next) {
            ('/', Some('/')) => {
                while i < bytes.len() && bytes[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == '*' && bytes[i + 1] == '/') {
                    if bytes[i] == '\n' {
                        out.push('\n');
                    }
                    i += 1;
                }
                i += 2;
            }
            ('"', _) | ('\'', _) => {
                out.push(c);
                i += 1;
                while i < bytes.len() {
                    out.push(bytes[i]);
                    if bytes[i] == '\\' {
                        if let Some(escaped) = bytes.get(i + 1) {
                            out.push(*escaped);
                        }
                        i += 2;
                        continue;
                    }
                    if bytes[i] == c {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The source of the named method, from its signature to the closing brace of
/// the body — matched by brace depth, so a nested block cannot end it early.
fn method_body(source: &str, signature: &str) -> String {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("GpuiTextInputView no longer declares `{signature}`"));
    let mut depth = 0usize;
    let mut entered = false;
    for (offset, ch) in source[start..].char_indices() {
        match ch {
            '{' => {
                depth += 1;
                entered = true;
            }
            '}' => {
                depth -= 1;
                if entered && depth == 0 {
                    return source[start..start + offset + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("`{signature}` has no closing brace");
}

/// The statement assigning `target`, from the target to its terminating `;`.
fn assignment(body: &str, target: &str) -> String {
    let start = body
        .find(target)
        .unwrap_or_else(|| panic!("no assignment to `{target}`:\n{body}"));
    let end = body[start..]
        .find(';')
        .unwrap_or_else(|| panic!("assignment to `{target}` is unterminated"));
    body[start..start + end].to_string()
}

/// Condition, then-arm and else-arm of the statement's conditional expression.
/// The arms are cut at the paren depth the `?` sits at, so a ternary nested in
/// a larger expression still yields its own three parts.
fn ternary(statement: &str) -> (String, String, String) {
    let chars: Vec<char> = statement.chars().collect();
    let question = chars.iter().position(|c| *c == '?').unwrap_or_else(|| {
        panic!("not a conditional expression — the branch is missing:\n{statement}")
    });

    let depth_at = |upto: usize| -> i32 {
        chars[..upto].iter().fold(0, |d, c| match c {
            '(' => d + 1,
            ')' => d - 1,
            _ => d,
        })
    };
    let level = depth_at(question);

    let mut cond_start = 0;
    for i in (0..question).rev() {
        if (chars[i] == '(' && depth_at(i) < level) || chars[i] == '=' {
            cond_start = i + 1;
            break;
        }
    }

    let mut colon = None;
    let mut arm_end = chars.len();
    for i in question + 1..chars.len() {
        let d = depth_at(i);
        if d < level {
            arm_end = i;
            break;
        }
        if chars[i] == ':' && d == level && colon.is_none() {
            colon = Some(i);
        }
    }
    let colon =
        colon.unwrap_or_else(|| panic!("conditional expression has no else-arm:\n{statement}"));

    let take = |range: std::ops::Range<usize>| chars[range].iter().collect::<String>();
    (
        take(cond_start..question),
        take(question + 1..colon),
        take(colon + 1..arm_end),
    )
}

/// The conditions of every `if` whose block contains `needle`, innermost first.
fn guards_around(body: &str, needle: &str) -> Vec<String> {
    let chars: Vec<char> = body.chars().collect();
    let mut guards: Vec<(usize, String)> = Vec::new();
    let mut search = 0;
    while let Some(found) = body[search..].find("if") {
        let keyword = search + found;
        search = keyword + 2;
        let Some(open) = body[keyword..].find('(').map(|o| keyword + o) else {
            continue;
        };
        if body[keyword + 2..open].chars().any(|c| !c.is_whitespace()) {
            continue;
        }
        let close = match_delimiter(&chars, open, '(', ')');
        let Some(block_open) = body[close..].find('{').map(|o| close + o) else {
            continue;
        };
        if body[close + 1..block_open]
            .chars()
            .any(|c| !c.is_whitespace())
        {
            continue;
        }
        let block_close = match_delimiter(&chars, block_open, '{', '}');
        let block: String = chars[block_open..block_close].iter().collect();
        if block.contains(needle) {
            let condition: String = chars[open + 1..close].iter().collect();
            guards.push((block.len(), condition));
        }
    }
    guards.sort_by_key(|(size, _)| *size);
    guards.into_iter().map(|(_, condition)| condition).collect()
}

fn match_delimiter(chars: &[char], open: usize, opener: char, closer: char) -> usize {
    let mut depth = 0usize;
    for (offset, ch) in chars[open..].iter().enumerate() {
        if *ch == opener {
            depth += 1;
        } else if *ch == closer {
            depth -= 1;
            if depth == 0 {
                return open + offset;
            }
        }
    }
    panic!("unbalanced `{opener}` at {open}");
}

/// The condition plus the initialiser of every local it names, so a test of a
/// condition reaches what the condition was computed from.
fn with_locals(condition: &str, body: &str) -> String {
    let mut resolved = condition.to_string();
    for word in condition.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if word.is_empty() {
            continue;
        }
        for declaration in ["boolean ", "int ", "final boolean ", "final int "] {
            let needle = format!("{declaration}{word} =");
            if let Some(at) = body.find(&needle) {
                let rest = &body[at + needle.len()..];
                let end = rest.find(';').unwrap_or(rest.len());
                resolved.push(' ');
                resolved.push_str(&rest[..end]);
            }
        }
    }
    resolved
}

/// An IME draws the button for whatever action `imeOptions` names, in place of
/// its Enter key. The multi-line arm must therefore ask for no action, and only
/// the single-line arm may ask for Done.
#[test]
fn ime_action_is_bound_to_the_single_line_arm() {
    let source = input_view_source();
    let body = method_body(
        &source,
        "public InputConnection onCreateInputConnection(EditorInfo outAttrs)",
    );
    let (condition, multi_line, single_line) = ternary(&assignment(&body, "outAttrs.imeOptions"));
    let condition = with_locals(&condition, &body);

    assert!(
        condition.contains("TYPE_TEXT_FLAG_MULTI_LINE"),
        "the imeOptions branch must be decided by the input type's multi-line flag, \
         but its condition is `{condition}`"
    );
    assert!(
        multi_line.contains("IME_ACTION_NONE") && !multi_line.contains("IME_ACTION_DONE"),
        "the multi-line arm must name IME_ACTION_NONE so the IME keeps its Enter key, \
         but it is `{multi_line}`"
    );
    assert!(
        single_line.contains("IME_ACTION_DONE") && !single_line.contains("IME_ACTION_NONE"),
        "the single-line arm must keep IME_ACTION_DONE, but it is `{single_line}`"
    );
}

/// A Return committed as text lands in the block as a literal newline and never
/// reaches gpui's keymap, so the editor's `Enter` action — the one that splits
/// the block — never runs. Every other commit must still be forwarded as text.
#[test]
fn a_committed_line_break_becomes_an_enter_keystroke() {
    let source = input_view_source();
    let body = method_body(
        &source,
        "public boolean commitText(CharSequence text, int newCursorPosition)",
    );

    let guards = guards_around(&body, "nativeKeyEnter");
    let innermost = guards.first().unwrap_or_else(|| {
        panic!("commitText sends every commit to Return, or none — no guard around it:\n{body}")
    });
    assert!(
        with_locals(innermost, &body).contains("isLineBreak"),
        "only a line break may become Return, but the guard is `{innermost}`"
    );

    let to_return = body.find("nativeKeyEnter").expect("guard found above");
    let as_text = body
        .find("nativeReplaceText")
        .expect("commitText must still forward ordinary text");
    assert!(
        to_return < as_text,
        "commitText tests for a line break only after forwarding the text:\n{body}"
    );

    let is_line_break = method_body(
        &source,
        "private static boolean isLineBreak(CharSequence text)",
    );
    for spelling in [r#""\n""#, r#""\r""#, r#""\r\n""#] {
        assert!(
            is_line_break.contains(spelling),
            "isLineBreak must recognise {spelling}, but reads:\n{is_line_break}"
        );
    }
}

/// The other route: IMEs that answer Return with a key event rather than a
/// commit. Both routes must reach the same split, exactly once per press.
#[test]
fn an_enter_key_event_becomes_an_enter_keystroke() {
    let source = input_view_source();
    let body = method_body(&source, "public boolean sendKeyEvent(KeyEvent event)");

    let guards = guards_around(&body, "nativeKeyEnter");
    assert!(
        !guards.is_empty(),
        "sendKeyEvent does not route Return to a split:\n{body}"
    );
    let resolved: Vec<String> = guards.iter().map(|g| with_locals(g, &body)).collect();

    assert!(
        resolved[0].contains("ACTION_DOWN"),
        "Return must split on the key-down only, or a press splits twice — \
         the innermost guard is `{}`",
        resolved[0]
    );
    for keycode in ["KEYCODE_ENTER", "KEYCODE_NUMPAD_ENTER"] {
        assert!(
            resolved.iter().any(|g| g.contains(keycode)),
            "no guard around the split tests {keycode}: {resolved:?}"
        );
    }
}
