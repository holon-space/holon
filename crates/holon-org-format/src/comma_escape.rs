//! Org's comma escape: a text line that org would read as syntax is written
//! with one more comma before that syntax, after the line's indentation, and
//! the reader removes one. A line that already has commas before that syntax
//! gets one more too, so every text round-trips.

#[derive(Clone, Copy, Debug)]
pub(crate) enum CommaEscape {
    /// A headline's text: what [`Self::Preamble`] escapes except a `#+ID:`
    /// line, which the parser keeps as text below a headline.
    Body,
    /// A page's text before its first headline: a line org reads as a
    /// headline (stars at the line's start, then a blank or the line's end)
    /// or a line whose text starts with `#+`, except the delimiters of a
    /// block the parser keeps as text (`#+begin_example`, `#+end_quote`,
    /// ...). `*bold*` at the start of a line stays as written.
    Preamble,
    /// The lines between `#+BEGIN_SRC` and `#+END_SRC`: a line starting with
    /// `*` or `#+`.
    Source,
}

impl CommaEscape {
    /// The byte where this codec's comma goes when it escapes `line`.
    fn comma_at(self, line: &str) -> Option<usize> {
        let indent = match self {
            Self::Body | Self::Preamble => line.len() - line.trim_start_matches([' ', '\t']).len(),
            Self::Source => 0,
        };
        let syntax = line[indent..].trim_start_matches(',');
        let guarded = match self {
            Self::Body => preamble_guards(indent, syntax) && !is_page_id_keyword(syntax),
            Self::Preamble => preamble_guards(indent, syntax),
            Self::Source => syntax.starts_with('*') || syntax.starts_with("#+"),
        };
        guarded.then_some(indent)
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
        text.split('\n')
            .map(|line| match self.comma_at(line) {
                Some(at) => guarded(line, at),
                None => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn preamble_guards(indent: usize, syntax: &str) -> bool {
    (indent == 0 && is_headline(syntax))
        || (syntax.starts_with("#+") && !is_text_block_delimiter(syntax))
}

pub(crate) fn is_page_id_keyword(line: &str) -> bool {
    line.trim_start().starts_with("#+ID:")
}

fn is_headline(line: &str) -> bool {
    let rest = line.trim_start_matches('*');
    rest.len() < line.len() && (rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// `#+begin_<name>` or `#+end_<name>` of any block but a source block: the
/// parser keeps such a block in the text of the block that holds it.
fn is_text_block_delimiter(line: &str) -> bool {
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
