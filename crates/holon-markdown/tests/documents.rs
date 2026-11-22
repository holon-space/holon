//! Shared enumerated documents for the adapter contract tests: every sequence
//! of up to `max_pieces` pieces, each a bullet or paragraph that declares at
//! most one block id the test knows. Exhaustive enumeration stands in for a
//! random generator: the space is small enough to cover in full.

use std::path::Path;

use holon_api::EntityUri;
use holon_core::file_format::FileFormatAdapter;
use holon_core::file_format::FileFormatParseResult;
use holon_markdown::LogseqMarkdownAdapter;
use holon_markdown::ObsidianMarkdownAdapter;

pub const ROOT: &str = "/vault";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Flavor {
    Logseq,
    Obsidian,
}

impl Flavor {
    pub const ALL: [Flavor; 2] = [Flavor::Logseq, Flavor::Obsidian];

    pub fn adapter(self) -> Box<dyn FileFormatAdapter> {
        match self {
            Flavor::Logseq => Box::new(LogseqMarkdownAdapter::new()),
            Flavor::Obsidian => Box::new(ObsidianMarkdownAdapter::new()),
        }
    }

    pub fn parse(self, source: &str) -> anyhow::Result<FileFormatParseResult> {
        let root = Path::new(ROOT);
        self.adapter()
            .parse(&root.join("P.md"), source, &EntityUri::no_parent(), root)
    }
}

/// One bullet or paragraph and the id it declares.
pub struct Piece {
    pub text: &'static str,
    pub declares: Option<&'static str>,
}

const fn piece(text: &'static str, declares: Option<&'static str>) -> Piece {
    Piece { text, declares }
}

const LOGSEQ_PIECES: &[Piece] = &[
    piece("- a", None),
    piece("- a\n  id:: x", Some("x")),
    piece("- b\n  id:: y", Some("y")),
    piece("  - c", None),
    piece("  - c\n    id:: x", Some("x")),
    piece("- {{query (todo TODO)}}", None),
    piece("- see ((a b))", None),
    piece("- see ((é)) and ((a|b))", None),
];

const OBSIDIAN_PIECES: &[Piece] = &[
    piece("a", None),
    piece("a ^x", Some("x")),
    piece("b ^y", Some("y")),
    piece("# H ^x", Some("x")),
    piece("- item ^y", Some("y")),
    piece("> [!note] n", None),
    piece("see ((a b)) and ((x\"y))", None),
];

const LOGSEQ_HEADERS: &[(&str, Option<&str>)] = &[
    ("", None),
    ("id:: x\n\n", Some("x")),
    ("title:: T\n\n", None),
];

/// A generated document, with the ids it declares in source order and the
/// 1-based line each is declared on.
pub struct Doc {
    pub source: String,
    pub declared: Vec<(&'static str, usize)>,
}

impl Doc {
    /// The first id declared twice, with both lines.
    pub fn first_duplicate(&self) -> Option<(&'static str, usize, usize)> {
        self.declared
            .iter()
            .enumerate()
            .find_map(|(n, (id, line))| {
                self.declared[..n]
                    .iter()
                    .find(|(first, _)| first == id)
                    .map(|(_, first_line)| (*id, *first_line, *line))
            })
    }
}

pub fn every_doc(flavor: Flavor, max_pieces: usize) -> Vec<Doc> {
    let (pieces, headers): (&[Piece], &[(&str, Option<&str>)]) = match flavor {
        Flavor::Logseq => (LOGSEQ_PIECES, LOGSEQ_HEADERS),
        Flavor::Obsidian => (OBSIDIAN_PIECES, &[("", None)]),
    };
    let mut docs = Vec::new();
    for len in 0..=max_pieces {
        for mut n in 0..pieces.len().pow(len as u32) {
            let chosen: Vec<&Piece> = (0..len)
                .map(|_| {
                    let p = &pieces[n % pieces.len()];
                    n /= pieces.len();
                    p
                })
                .collect();
            for (header, header_id) in headers {
                let mut source = header.to_string();
                let mut declared: Vec<(&'static str, usize)> =
                    header_id.map(|id| (id, 1)).into_iter().collect();
                let mut line = header.lines().count();
                for p in &chosen {
                    if let Some(id) = p.declares {
                        declared.push((id, line + 1));
                    }
                    source.push_str(p.text);
                    source.push_str(match flavor {
                        Flavor::Logseq => "\n",
                        Flavor::Obsidian => "\n\n",
                    });
                    line += p.text.lines().count() + usize::from(flavor == Flavor::Obsidian);
                }
                docs.push(Doc { source, declared });
            }
        }
    }
    docs
}
