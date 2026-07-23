//! The fixed tabular schema shared by the renderer and parser.
//!
//! The whole representation is a *single* TOON tabular array. Four fields that
//! are near-universal get their own column (`id`, `depth`, `state`, plus the
//! two text slots `body`/`title`); everything rarer is folded into the `props`
//! cell (see `toon.rs`). This keeps the common row — a bare DONE task — as
//! narrow as possible while staying lossless.

/// Leaf columns, in row order.
pub const COLUMNS: [&str; 6] = ["id", "depth", "state", "props", "body", "title"];

pub const N_COLUMNS: usize = COLUMNS.len();

/// The array key.
pub const TABLE_KEY: &str = "blocks";

/// Rows are indented one TOON level (2 spaces) below the header.
pub const ROW_INDENT: &str = "  ";

// Reserved props keys carry a leading `@` sigil so they can NEVER collide with
// an arbitrary org drawer key. Real drawer keys (`assigned-to`, `Effort`,
// `REQUIRES`, `source-file`, …) are alphanumeric/dash and never start with `@`,
// so the parser routes any `@`-prefixed key to a typed field and every bare key
// to the arbitrary property map — collision-free by construction.
pub const K_PRI: &str = "@pri";
pub const K_TAGS: &str = "@tags";
pub const K_KIND: &str = "@kind";
pub const K_LANG: &str = "@lang";
pub const K_NAME: &str = "@name";
pub const K_SCHED: &str = "@sched";
pub const K_DEADLINE: &str = "@dead";
pub const K_REQUIRES: &str = "@req";
pub const K_ADVICE: &str = "@adv";
pub const K_COLLAPSED: &str = "@col";

pub const KIND_SRC: &str = "src";
pub const KIND_IMG: &str = "img";

/// The exact header line for an `n`-row table.
pub fn header_line(n: usize) -> String {
    format!("{}[{}]{{{}}}:", TABLE_KEY, n, COLUMNS.join(","))
}
