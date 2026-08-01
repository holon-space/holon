//! Syntax-neutral inline scanner for the markdown flavors (Obsidian + LogSeq).
//!
//! Produces a `(content, marks, tags)` triple from one block's raw inline text:
//! `content` is the display text with delimiters stripped, `marks` are
//! `MarkSpan`s over **Unicode-scalar offsets** into `content` (matching Loro's
//! `LoroText::mark` API and the SQL `marks` column), and `tags` are the
//! `#tag` / `#[[multi word]]` names hoisted to the block's tag edge field.
//!
//! Parse, don't validate: the scanner is total. Anything it does not recognize
//! is emitted verbatim into `content` (never dropped) — the fail-loud
//! disclosure for *whole-block* unsupported constructs (callouts, embeds,
//! queries) happens one layer up in the block parsers.

use holon_api::EntityUri;
use holon_api::inline_mark::EntityRef;
use holon_api::inline_mark::InlineMark;
use holon_api::inline_mark::MarkSpan;

/// Result of scanning one block's inline text.
pub struct InlineParse {
    pub content: String,
    pub marks: Vec<MarkSpan>,
    /// `#tag` / `#[[multi word]]` names, in first-seen order.
    pub tags: Vec<String>,
}

struct Scanner {
    src: Vec<char>,
    i: usize,
    out: String,
    /// Char length of `out` so far (marks index into this space).
    out_len: usize,
    marks: Vec<MarkSpan>,
    tags: Vec<String>,
}

impl Scanner {
    fn push_str(&mut self, s: &str) {
        for c in s.chars() {
            self.out.push(c);
            self.out_len += 1;
        }
    }

    /// Find the char index of the next occurrence of `needle` at or after
    /// `from`.
    fn find(&self, needle: &[char], from: usize) -> Option<usize> {
        if needle.is_empty() || from + needle.len() > self.src.len() {
            return None;
        }
        let mut j = from;
        while j + needle.len() <= self.src.len() {
            if self.src[j..j + needle.len()] == *needle {
                return Some(j);
            }
            j += 1;
        }
        None
    }

    fn starts_with(&self, needle: &[char]) -> bool {
        self.i + needle.len() <= self.src.len()
            && self.src[self.i..self.i + needle.len()] == *needle
    }

    /// Emit `label` as content and record a mark of `mark` over its range.
    fn emit_marked(&mut self, label: &str, mark: InlineMark) {
        let start = self.out_len;
        self.push_str(label);
        let end = self.out_len;
        if end > start {
            self.marks.push(MarkSpan::new(start, end, mark));
        }
    }

    /// A `#tag` is a tag only at a word boundary (start of block or after
    /// whitespace / an opening bracket), never mid-word (`C#`, `foo#bar`).
    fn at_tag_boundary(&self) -> bool {
        if self.i == 0 {
            return true;
        }
        let prev = self.src[self.i - 1];
        prev.is_whitespace() || prev == '(' || prev == '['
    }
}

/// Delimiter pairs for symmetric inline emphasis, longest-first so `**` wins
/// over `*` and `~~` over a lone `~`.
/// One symmetric emphasis delimiter and the mark it opens.
type EmphasisPair = (&'static str, fn() -> InlineMark);

const EMPHASIS: &[EmphasisPair] = &[
    ("**", || InlineMark::Bold),
    ("__", || InlineMark::Bold),
    ("~~", || InlineMark::Strike),
    ("==", || InlineMark::Underline), // highlight → Underline (spike; O3 open question)
    ("*", || InlineMark::Italic),
    ("_", || InlineMark::Italic),
    ("`", || InlineMark::Code),
];

/// Scan one block's inline text.
pub fn scan_inline(text: &str) -> InlineParse {
    let mut s = Scanner {
        src: text.chars().collect(),
        i: 0,
        out: String::new(),
        out_len: 0,
        marks: Vec::new(),
        tags: Vec::new(),
    };

    while s.i < s.src.len() {
        // --- Embed `![[...]]` / `![alt](url)` : keep verbatim, no mark. The
        //     whole-block opaque path handles standalone embeds; inline we
        //     preserve the exact source so nothing is lost.
        if s.starts_with(&['!', '[', '[']) {
            s.push_str("![[");
            s.i += 3;
            continue;
        }

        // --- Wikilink `[[target]]` / `[[target|alias]]` / `[[target#h]]`
        if s.starts_with(&['[', '[']) {
            if let Some(close) = s.find(&[']', ']'], s.i + 2) {
                let inner: String = s.src[s.i + 2..close].iter().collect();
                let (target, label) = split_wikilink(&inner);
                let uri = wikilink_target(&target);
                s.emit_marked(
                    &label,
                    InlineMark::Link {
                        target: uri,
                        label: label.clone(),
                    },
                );
                s.i = close + 2;
                continue;
            }
        }

        // --- Block ref `((uuid))`
        if s.starts_with(&['(', '(']) {
            if let Some(close) = s.find(&[')', ')'], s.i + 2) {
                let inner: String = s.src[s.i + 2..close].iter().collect();
                let inner = inner.trim().to_string();
                if !inner.is_empty() {
                    let target = EntityRef::from_uri(&EntityUri::block(&inner));
                    let label = format!("(({inner}))");
                    s.emit_marked(
                        &label,
                        InlineMark::Link {
                            target,
                            label: label.clone(),
                        },
                    );
                    s.i = close + 2;
                    continue;
                }
            }
        }

        // --- Markdown link `[text](url)`
        if s.starts_with(&['[']) && !s.starts_with(&['[', '[']) {
            if let Some(rbrack) = s.find(&[']'], s.i + 1) {
                if rbrack + 1 < s.src.len() && s.src[rbrack + 1] == '(' {
                    if let Some(rparen) = s.find(&[')'], rbrack + 2) {
                        let label: String = s.src[s.i + 1..rbrack].iter().collect();
                        let url: String = s.src[rbrack + 2..rparen].iter().collect();
                        let target = md_link_target(&url);
                        s.emit_marked(
                            &label,
                            InlineMark::Link {
                                target,
                                label: label.clone(),
                            },
                        );
                        s.i = rparen + 1;
                        continue;
                    }
                }
            }
        }

        // --- Tag `#[[multi word]]` or `#tag`
        if s.src[s.i] == '#' && s.at_tag_boundary() {
            if s.i + 2 < s.src.len() && s.src[s.i + 1] == '[' && s.src[s.i + 2] == '[' {
                if let Some(close) = s.find(&[']', ']'], s.i + 3) {
                    let name: String = s.src[s.i + 3..close].iter().collect();
                    if !name.trim().is_empty() {
                        push_tag(&mut s.tags, name.trim());
                        // Preserve the literal source so the display text is lossless.
                        let lit: String = s.src[s.i..close + 2].iter().collect();
                        s.push_str(&lit);
                        s.i = close + 2;
                        continue;
                    }
                }
            }
            // bare `#tag`
            let mut j = s.i + 1;
            while j < s.src.len() && is_tag_char(s.src[j]) {
                j += 1;
            }
            let name: String = s.src[s.i + 1..j].iter().collect();
            if is_valid_tag(&name) {
                push_tag(&mut s.tags, &name);
                let lit: String = s.src[s.i..j].iter().collect();
                s.push_str(&lit);
                s.i = j;
                continue;
            }
        }

        // --- Emphasis / code
        let mut matched = false;
        for (delim, mk) in EMPHASIS {
            let dch: Vec<char> = delim.chars().collect();
            if s.starts_with(&dch) {
                if let Some(close) = s.find(&dch, s.i + dch.len()) {
                    let inner: String = s.src[s.i + dch.len()..close].iter().collect();
                    if !inner.is_empty() {
                        s.emit_marked(&inner, mk());
                        s.i = close + dch.len();
                        matched = true;
                        break;
                    }
                }
            }
        }
        if matched {
            continue;
        }

        // --- Plain char
        s.out.push(s.src[s.i]);
        s.out_len += 1;
        s.i += 1;
    }

    InlineParse {
        content: s.out,
        marks: s.marks,
        tags: s.tags,
    }
}

fn split_wikilink(inner: &str) -> (String, String) {
    // `target|alias` (Obsidian/LogSeq alias). Label defaults to target, minus
    // any `#heading` / `#^block` anchor, which is a resolution hint not display.
    if let Some((t, a)) = inner.split_once('|') {
        (t.trim().to_string(), a.trim().to_string())
    } else {
        let label = inner.split('#').next().unwrap_or(inner).trim().to_string();
        let label = if label.is_empty() {
            inner.trim().to_string()
        } else {
            label
        };
        (inner.trim().to_string(), label)
    }
}

fn wikilink_target(target: &str) -> EntityRef {
    // Strip anchor for the name; dangling by default (lazy page-create,
    // links-ruling).
    let name = target.split('#').next().unwrap_or(target).trim();
    EntityRef::Name {
        name: name.to_string(),
    }
}

fn md_link_target(url: &str) -> EntityRef {
    let u = url.trim();
    if u.starts_with("http://") || u.starts_with("https://") || u.starts_with("mailto:") {
        EntityRef::External { url: u.to_string() }
    } else {
        // A relative markdown link into the vault — treat as a dangling name ref.
        let name = u.trim_end_matches(".md").trim_end_matches(".org");
        EntityRef::Name {
            name: name.to_string(),
        }
    }
}

fn is_tag_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-' || c == '/'
}

fn is_valid_tag(name: &str) -> bool {
    // Obsidian rule: non-empty and contains at least one non-numeric char.
    !name.is_empty() && name.chars().any(|c| !c.is_ascii_digit())
}

fn push_tag(tags: &mut Vec<String>, name: &str) {
    let n = name.to_string();
    if !tags.contains(&n) {
        tags.push(n);
    }
}
