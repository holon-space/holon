//! Token measurement: org (full) vs org (ID-compressed) vs TOON, on real vault
//! files, for both the full block set and the "exclude DONE" projection.
//!
//! Run: `cargo run -p holon-toon --example measure --features measure --
//! <file.org> [more.org ...]`
//!
//! Tokenizer: `tiktoken-rs` `o200k_base` (the GPT-4o/o200k BPE). Claude uses a
//! different tokenizer, but o200k is a well-correlated public proxy for
//! English + code and is fully reproducible; treat the absolute counts as
//! representative and the *ratios* as the finding.

use holon_toon::Forest;
use holon_toon::org_reader;
use holon_toon::render;
use tiktoken_rs::o200k_base;

struct Row {
    label: String,
    org_full: usize,
    org_comp: usize,
    toon: usize,
    blocks: usize,
    // tokens consumed by the bare block ids alone — the irreducible cost BOTH
    // formats must pay once per block.
    id_tokens: usize,
    // round-trip check for the TOON rendering
    toon_roundtrips: bool,
}

fn id_tokens(bpe: &tiktoken_rs::CoreBPE, forest: &Forest) -> usize {
    forest
        .flatten()
        .iter()
        .map(|(_, b)| count(bpe, b.id.as_str()))
        .sum()
}

fn count(bpe: &tiktoken_rs::CoreBPE, s: &str) -> usize {
    bpe.encode_with_special_tokens(s).len()
}

fn measure(bpe: &tiktoken_rs::CoreBPE, label: &str, forest: &Forest) -> Row {
    let org_full = org_reader::render_org_full(forest);
    let org_comp = org_reader::render_org_compressed(forest);
    let toon = render(forest);

    let roundtrips = match holon_toon::parse(&toon) {
        Ok(parsed) => &parsed == forest,
        Err(_) => false,
    };

    Row {
        label: label.to_string(),
        org_full: count(bpe, &org_full),
        org_comp: count(bpe, &org_comp),
        toon: count(bpe, &toon),
        blocks: forest.block_count(),
        id_tokens: id_tokens(bpe, forest),
        toon_roundtrips: roundtrips,
    }
}

fn pct(base: usize, other: usize) -> f64 {
    if base == 0 {
        return 0.0;
    }
    100.0 * (base as f64 - other as f64) / base as f64
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: measure <file.org> [more.org ...]");
        std::process::exit(2);
    }
    let bpe = o200k_base().expect("load o200k_base tokenizer");

    let mut rows: Vec<Row> = Vec::new();
    for path in &args {
        let src = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {}", path, e));
        let forest = org_reader::parse_org(&src);
        let name = std::path::Path::new(path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());

        rows.push(measure(&bpe, &format!("{} (all)", name), &forest));
        let filtered = org_reader::filter_exclude_done(&forest);
        rows.push(measure(&bpe, &format!("{} (no DONE)", name), &filtered));
    }

    println!(
        "{:<34} {:>7} {:>9} {:>9} {:>7} {:>8} {:>8} {:>8} {:>5}",
        "dataset", "blocks", "org_full", "org_comp", "toon", "vs_full", "vs_comp", "ids", "rt"
    );
    println!("{}", "-".repeat(104));
    for r in &rows {
        println!(
            "{:<34} {:>7} {:>9} {:>9} {:>7} {:>7.1}% {:>7.1}% {:>7} {:>5}",
            r.label,
            r.blocks,
            r.org_full,
            r.org_comp,
            r.toon,
            pct(r.org_full, r.toon),
            pct(r.org_comp, r.toon),
            r.id_tokens,
            if r.toon_roundtrips { "ok" } else { "FAIL" },
        );
    }
    println!(
        "\nvs_full = % tokens saved by TOON vs canonical org; \
         vs_comp = vs ID-compressed org.\n\
         ids = tokens spent on bare block ids alone (irreducible; paid once per \
         block by EVERY format).\n\
         rt = TOON parse(render)==forest. Tokenizer: tiktoken o200k_base."
    );
}
