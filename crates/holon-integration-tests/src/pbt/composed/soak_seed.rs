//! Scale-soak vault seeder (env-gated, zero-cost when off).
//!
//! The keystone (`general_e2e_composed_pbt`) boots a 3-block focus doc — vault-scale
//! behaviour (the projection/consolidator latency cliff `pass_ms ≈ 11.3 + 0.221×blocks`,
//! CDC cost at 5–10k blocks, RSS growth) never manifests, so Martin discovers it by
//! hand. This module inflates ONLY the SUT boot with extra org **doc files** — realistic
//! synthetic pages (deep trees, tasks, links, unicode). They land in the SUT store as
//! separate documents, so `boot_and_seed_wide`'s scaffold math folds their ids into the
//! oracle as seed blocks (`block_documents[id]=no_parent`, exactly like `block:journals`
//! and the boot layout) — they drop out of the block comparison and the focus-root
//! candidate set, so the invariant catalog stays green while every action still pays the
//! whole-vault projection / CDC / consolidator cost.
//!
//! Everything here is a pure function of `HOLON_SOAK_SEED_BLOCKS` (deterministic — same
//! vault every run). When it is unset or `0`, [`soak_org_files`] returns empty and the
//! keystone boots exactly as before.

use std::time::Duration;

/// Total number of extra vault blocks to seed. `HOLON_SOAK_SEED_BLOCKS` (default `0` —
/// soak off, keystone behaviour unchanged).
pub fn soak_block_count() -> usize {
    std::env::var("HOLON_SOAK_SEED_BLOCKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Blocks per synthetic doc file. `HOLON_SOAK_BLOCKS_PER_DOC` (default `200`) — a real
/// vault is many pages, so the seed is split across `ceil(N / this)` doc files.
fn blocks_per_doc() -> usize {
    std::env::var("HOLON_SOAK_BLOCKS_PER_DOC")
        .ok()
        .and_then(|s| s.parse().ok())
        .filter(|&n: &usize| n > 0)
        .unwrap_or(200)
}

/// The per-action convergence-settle budget. `HOLON_SOAK_SETTLE_MS` (default = the
/// keystone's `wide_e2e::SETTLE` = 150ms). MUST be raised for a scale run: at 5–10k
/// blocks the projection/CDC drain far exceeds 150ms, and a too-small budget makes
/// `converge_projections` return BEFORE quiescence — which both risks stale-read
/// invariant flakes AND caps the measured `action_total` below the true latency (it
/// would hide the very cliff the soak exists to quantify).
pub fn soak_settle() -> Duration {
    match std::env::var("HOLON_SOAK_SETTLE_MS")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        Some(ms) => Duration::from_millis(ms),
        None => super::wide_e2e::SETTLE,
    }
}

/// Deterministic pseudo-random stream (splitmix64) — variety without a proptest RNG, so
/// the vault is byte-identical across runs at a given size.
fn splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A pool of unicode-rich content fragments, exercising multi-byte parsing at scale.
const FRAGMENTS: &[&str] = &[
    "cafe resume naive — grapheme edge cases",
    "日本語のテキスト — CJK wide chars",
    "Ελληνικά κείμενο — Greek",
    "Кириллица — Cyrillic sample",
    "emoji run 🚀🔥🧠📚✅ — astral plane",
    "mixed العربية RTL segment",
    "math ∑∫∂∇ ≠ ≤ ≥ ∞ — symbols",
    "plain ascii note for baseline width",
];

/// Generate the synthetic vault as org doc files: `Vec<(filename, content)>`. Empty when
/// the soak is off. Each doc is a page (`#+ID: soak-doc-K`) of headings with a stable
/// `:ID:` drawer (`soak-K-J`), deterministic depth (deep trees), task markers, unicode
/// content, and occasional intra-vault links.
pub fn soak_org_files() -> Vec<(String, String)> {
    let total = soak_block_count();
    if total == 0 {
        return Vec::new();
    }
    let per_doc = blocks_per_doc();
    let doc_count = total.div_ceil(per_doc);
    let mut files = Vec::with_capacity(doc_count);
    let mut emitted = 0usize;

    for k in 0..doc_count {
        let mut body = format!("#+ID: soak-doc-{k}\n#+TITLE: Soak Page {k}\n");
        let mut rng = 0x5EED_0000_0000_0000u64 ^ (k as u64);
        // Current heading depth (org `*` count). Random-walk in 1..=4, biased to stay
        // shallow, so the tree is genuinely nested (parents + descendants) not flat.
        let mut depth: usize = 1;
        for j in 0..per_doc {
            if emitted >= total {
                break;
            }
            emitted += 1;
            let r = splitmix(&mut rng);
            // Walk depth: 40% deeper, 30% shallower, 30% same — clamped to 1..=4, and
            // never deeper than one below the previous (valid org nesting).
            depth = match r % 10 {
                0..=3 => (depth + 1).min(4),
                4..=6 => depth.saturating_sub(1).max(1),
                _ => depth,
            };
            let stars = "*".repeat(depth);
            let marker = match (r >> 8) % 6 {
                0 => "TODO ",
                1 => "DONE ",
                2 => "DOING ",
                _ => "",
            };
            let frag = FRAGMENTS[((r >> 16) as usize) % FRAGMENTS.len()];
            // Every ~7th block carries an intra-vault link, exercising link parsing.
            let link = if j % 7 == 3 && j > 0 {
                format!(" see [[block:soak-{k}-0][top]]")
            } else {
                String::new()
            };
            body.push_str(&format!(
                "{stars} {marker}{frag} · block {k}-{j}{link}\n\
                 :PROPERTIES:\n:ID: soak-{k}-{j}\n:END:\n"
            ));
        }
        files.push((format!("soak-{k}.org"), body));
    }
    files
}
