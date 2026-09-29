//! Org's comma escape. Org removes one comma before `*` or `#+` (after the
//! line's indentation and any other commas) only inside example, export and
//! source blocks (`org-unescape-code-in-string`); everywhere else a comma is
//! text. Holon writes that escape inside those blocks, the D230.a escape in
//! paragraph text, and inside other blocks and drawers only before a headline.

#[derive(Clone, Copy, Debug)]
pub(crate) enum CommaEscape {
    /// A headline's text: what [`Self::Preamble`] escapes except a `#+ID:`
    /// line, which the parser keeps as text below a headline.
    Body,
    /// A page's text before its first headline. In paragraph text: a line org
    /// reads as a headline (stars at the line's start, then a blank or the
    /// line's end) or a line whose text starts with `#+`, except the
    /// delimiters of a block the parser keeps as text (`#+begin_example`,
    /// `#+end_quote`, ...). `*bold*` at the start of a line stays as written.
    Preamble,
    /// The lines between `#+BEGIN_SRC` and `#+END_SRC`.
    Source,
}

/// How org reads a comma at the start of a text line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Context {
    /// Paragraph text, outside any block or drawer.
    Paragraph,
    /// Inside an example, export or source block: org removes one comma.
    Unescaped,
    /// Inside any other block or a drawer: a comma is text, and a headline
    /// ends the block, so Holon escapes only a headline.
    Kept,
}

impl CommaEscape {
    /// The byte where this codec's comma goes when it escapes `line`, a line
    /// in `context`.
    fn comma_at(self, line: &str, context: Context) -> Option<usize> {
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        let syntax = line[indent..].trim_start_matches(',');
        let guarded = match context {
            Context::Kept => indent == 0 && is_headline(syntax),
            Context::Unescaped => syntax.starts_with('*') || syntax.starts_with("#+"),
            Context::Paragraph => self.paragraph_guards(indent, syntax),
        };
        guarded.then_some(indent)
    }

    fn paragraph_guards(self, indent: usize, syntax: &str) -> bool {
        match self {
            Self::Body => preamble_guards(indent, syntax) && !is_page_id_keyword(syntax),
            Self::Preamble => preamble_guards(indent, syntax),
            Self::Source => unreachable!("source lines are never paragraph text"),
        }
    }

    /// `text` with the byte where each escaped line's comma goes replaced by
    /// a comma: org reads each such line as paragraph text, the way it reads
    /// the escaped line in a file, and every offset into `text` stays valid.
    pub(crate) fn inert(self, text: &str) -> String {
        self.map_lines(text, |line, at| {
            format!("{},{}", &line[..at], &line[at + 1..])
        })
    }

    pub(crate) fn escape(self, text: &str) -> String {
        self.map_lines(text, |line, at| format!("{},{}", &line[..at], &line[at..]))
    }

    pub(crate) fn unescape(self, text: &str) -> String {
        self.map_lines(text, |line, at| match line[at..].strip_prefix(',') {
            Some(rest) => format!("{}{rest}", &line[..at]),
            None => line.to_string(),
        })
    }

    fn map_lines(self, text: &str, guarded: impl Fn(&str, usize) -> String) -> String {
        let lines: Vec<&str> = text.split('\n').collect();
        let contexts = match self {
            Self::Body | Self::Preamble => self.contexts(&lines),
            Self::Source => vec![Context::Unescaped; lines.len()],
        };
        lines
            .iter()
            .zip(contexts)
            .map(|(line, context)| match self.comma_at(line, context) {
                Some(at) => guarded(line, at),
                None => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The context of each line of paragraph-level text. A line opens a block
    /// or drawer only where this codec writes it without a comma, so the
    /// escaped and the unescaped text have the same contexts.
    fn contexts(self, lines: &[&str]) -> Vec<Context> {
        let mut contexts = Vec::with_capacity(lines.len());
        let mut open: Vec<Container> = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if open.last().is_some_and(|c| c.closes(line)) {
                open.pop();
                contexts.push(open.last().map_or(Context::Paragraph, |c| c.context()));
                continue;
            }
            let context = open.last().map_or(Context::Paragraph, |c| c.context());
            contexts.push(context);
            if context == Context::Unescaped
                || matches!(open.last(), Some(c) if c.literal())
                || (context == Context::Paragraph && self.comma_at(line, context).is_some())
            {
                continue;
            }
            if let Some(container) =
                Container::opened_by(line).filter(|c| lines[i + 1..].iter().any(|l| c.closes(l)))
            {
                open.push(container);
            }
        }
        contexts
    }
}

/// A block or drawer that holds text lines.
enum Container {
    Block(String),
    Dynamic,
    Drawer,
}

impl Container {
    fn opened_by(line: &str) -> Option<Self> {
        let t = line.trim();
        let lower = t.to_ascii_lowercase();
        if let Some(rest) = lower.strip_prefix("#+begin_") {
            let name = rest.split_whitespace().next().unwrap_or("");
            return (!name.is_empty()).then(|| Self::Block(name.to_string()));
        }
        if lower.starts_with("#+begin:") {
            return Some(Self::Dynamic);
        }
        let name = t.strip_prefix(':')?.strip_suffix(':')?;
        (!name.is_empty()
            && !name.eq_ignore_ascii_case("END")
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '-'))
        .then_some(Self::Drawer)
    }

    fn closes(&self, line: &str) -> bool {
        let lower = line.trim().to_ascii_lowercase();
        match self {
            Self::Block(name) => lower
                .strip_prefix("#+end_")
                .is_some_and(|rest| rest.split_whitespace().next() == Some(name.as_str())),
            Self::Dynamic => lower == "#+end:",
            Self::Drawer => lower == ":end:",
        }
    }

    /// Whether org reads the lines inside as they are, with no elements.
    fn literal(&self) -> bool {
        matches!(self, Self::Block(name) if ["example", "export", "src", "verse", "comment"].contains(&name.as_str()))
    }

    fn context(&self) -> Context {
        match self {
            Self::Block(name) if ["example", "export", "src"].contains(&name.as_str()) => {
                Context::Unescaped
            }
            _ => Context::Kept,
        }
    }
}

fn preamble_guards(indent: usize, syntax: &str) -> bool {
    (indent == 0 && is_headline(syntax))
        || (syntax.starts_with("#+") && !is_text_block_delimiter(syntax))
}

/// The value of a `#+ID:` keyword line, any case, as org reads keywords.
pub(crate) fn page_id_keyword_value(line: &str) -> Option<&str> {
    let line = line.trim();
    line.get(..5)
        .filter(|key| key.eq_ignore_ascii_case("#+ID:"))
        .map(|_| line[5..].trim())
}

pub(crate) fn is_page_id_keyword(line: &str) -> bool {
    page_id_keyword_value(line).is_some()
}

fn is_headline(line: &str) -> bool {
    let rest = line.trim_start_matches('*');
    rest.len() < line.len() && (rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// `#+BEGIN: name` or `#+END:`, the delimiters of a dynamic block, which the
/// parser keeps as text.
pub(crate) fn is_dynamic_block_delimiter(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    lower.starts_with("#+begin:") || lower == "#+end:"
}

/// `#+begin_<name>` or `#+end_<name>` of any block but a source block, or a
/// dynamic block's delimiter: the parser keeps such a block in the text of the
/// block that holds it.
fn is_text_block_delimiter(line: &str) -> bool {
    if is_dynamic_block_delimiter(line) {
        return true;
    }
    let lower = line.to_ascii_lowercase();
    let Some(rest) = lower
        .strip_prefix("#+begin_")
        .or_else(|| lower.strip_prefix("#+end_"))
    else {
        return false;
    };
    let name = rest.split_whitespace().next().unwrap_or("");
    !name.is_empty() && name != "src"
}

#[cfg(test)]
mod tests {
    use super::CommaEscape;

    #[test]
    fn unescape_inverts_escape() {
        let text = "* a\n,* b\n,,* c\n#+x\n,#+y\n#+begin_example\n,#+end_example\n,plain\nplain\n\n*\n  #+x\n  ,#+y\n  * item\n\t#+ID: q";
        for codec in [
            CommaEscape::Body,
            CommaEscape::Preamble,
            CommaEscape::Source,
        ] {
            assert_eq!(codec.unescape(&codec.escape(text)), text, "{codec:?}");
        }
    }

    #[test]
    fn inert_keeps_every_offset() {
        let text = "* a\n#+x: y\n,* b\n#+ID: p\n  #+x\n#+begin_example\n*bold*\né";
        let inert = CommaEscape::Preamble.inert(text);
        assert_eq!(
            inert,
            ", a\n,+x: y\n,* b\n,+ID: p\n  ,+x\n#+begin_example\n*bold*\né"
        );
        assert_eq!(inert.len(), text.len());
    }
}
