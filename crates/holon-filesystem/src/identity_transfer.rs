//! Identity transfer: binds the id-less headlines of a fresh parse to the store
//! ids of the file's bound base snapshot, so an ingest diffs store ids against
//! store ids instead of against freshly minted ones.
//!
//! An authored `:ID:` always wins. The remaining headlines are aligned against
//! the unclaimed base headlines by a longest-common-subsequence diff on
//! `(depth, text)`; equal runs bind by position. A headline whose text is
//! unique among the leftovers on both sides is a move and binds next. Inside a
//! replace hunk the [`PairStrategy`] decides which edited headline keeps which
//! base id. Anything left over is an insert (fresh id) or a deletion (unbound
//! base id).

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::EntityUri;

#[derive(Debug, Clone)]
pub struct BaseHeadline {
    pub id: EntityUri,
    pub depth: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct ParsedHeadline {
    pub id: Option<EntityUri>,
    pub depth: usize,
    pub text: String,
}

#[derive(Debug, Clone, Copy)]
pub struct TransferPolicy {
    /// Bind exact-text moves before any replace hunk is paired.
    pub exact_moves_first: bool,
    pub pairing: PairStrategy,
}

#[derive(Debug, Clone, Copy)]
pub enum PairStrategy {
    /// The i-th base headline of a replace hunk pairs with its i-th parsed one
    /// when depth and parent agree.
    Positional,
    /// Mutual best character-bigram similarity at or above `threshold`; a tie
    /// on either side leaves both unpaired.
    Similarity {
        threshold: f64,
        structure: Structure,
        /// After the hunks, pair the document-wide leftovers by text alone
        /// (an edited headline that also moved).
        document_leftovers: bool,
    },
}

/// What a hunk pair must share besides similar text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Structure {
    DepthAndParent,
    Depth,
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindSource {
    Equal,
    ExactMove,
    Hunk,
    DocumentLeftover,
}

/// Per parsed headline: how it got its id. `None` for an authored id or a
/// headline left unbound (a fresh insert).
pub type Bindings = Vec<Option<BindSource>>;

pub fn transfer_ids(
    base: &[BaseHeadline],
    parsed: &mut [ParsedHeadline],
    policy: TransferPolicy,
) -> Bindings {
    let mut sources: Bindings = vec![None; parsed.len()];
    let authored: Vec<bool> = parsed.iter().map(|p| p.id.is_some()).collect();
    let mut claimed: HashSet<EntityUri> = parsed.iter().filter_map(|p| p.id.clone()).collect();
    let mut bind = |parsed: &mut [ParsedHeadline], b: usize, p: usize, source: BindSource| {
        assert!(
            claimed.insert(base[b].id.clone()),
            "{source:?} bound base id {} twice",
            base[b].id
        );
        parsed[p].id = Some(base[b].id.clone());
        sources[p] = Some(source);
    };

    let base_parent = base_parents(base);
    let free_base: Vec<usize> = (0..base.len())
        .filter(|&i| !parsed.iter().any(|p| p.id.as_ref() == Some(&base[i].id)))
        .collect();
    let free_parsed: Vec<usize> = (0..parsed.len())
        .filter(|&j| parsed[j].id.is_none())
        .collect();

    let anchors = lcs(&free_base, &free_parsed, |b, p| {
        base[b].depth == parsed[p].depth && base[b].text == parsed[p].text
    });
    for &(bi, pj) in &anchors {
        bind(parsed, free_base[bi], free_parsed[pj], BindSource::Equal);
    }

    let mut hunks = Vec::new();
    let (mut bi, mut pj) = (0, 0);
    for &(ab, ap) in anchors
        .iter()
        .chain(std::iter::once(&(free_base.len(), free_parsed.len())))
    {
        if ab > bi || ap > pj {
            hunks.push((free_base[bi..ab].to_vec(), free_parsed[pj..ap].to_vec()));
        }
        bi = ab + 1;
        pj = ap + 1;
    }

    if policy.exact_moves_first {
        let open_b: Vec<usize> = hunks.iter().flat_map(|h| h.0.clone()).collect();
        let open_p: Vec<usize> = hunks.iter().flat_map(|h| h.1.clone()).collect();
        for &p in &open_p {
            let text = &parsed[p].text;
            let twins_p = open_p.iter().filter(|&&x| &parsed[x].text == text).count();
            let twins_b: Vec<usize> = open_b
                .iter()
                .copied()
                .filter(|&b| &base[b].text == text)
                .collect();
            if let (&[b], 1) = (&twins_b[..], twins_p) {
                bind(parsed, b, p, BindSource::ExactMove);
            }
        }
        for h in &mut hunks {
            h.0.retain(|&b| !parsed.iter().any(|p| p.id.as_ref() == Some(&base[b].id)));
            h.1.retain(|&p| parsed[p].id.is_none());
        }
    }

    for (hb, hp) in &hunks {
        match policy.pairing {
            PairStrategy::Positional => {
                for (&b, &p) in hb.iter().zip(hp) {
                    // Bound in order so a child later in the hunk sees its parent.
                    if fits(Structure::DepthAndParent, base, &base_parent, parsed, b, p) {
                        bind(parsed, b, p, BindSource::Hunk);
                    }
                }
            }
            PairStrategy::Similarity {
                threshold,
                structure,
                ..
            } => {
                for (b, p) in pair_similar(base, &base_parent, parsed, hb, hp, threshold, structure)
                {
                    bind(parsed, b, p, BindSource::Hunk);
                }
            }
        }
    }

    if let PairStrategy::Similarity {
        threshold,
        document_leftovers: true,
        ..
    } = policy.pairing
    {
        let open_b: Vec<usize> = (0..base.len())
            .filter(|&b| !parsed.iter().any(|p| p.id.as_ref() == Some(&base[b].id)))
            .collect();
        let open_p: Vec<usize> = (0..parsed.len())
            .filter(|&p| parsed[p].id.is_none())
            .collect();
        for (b, p) in pair_similar(
            base,
            &base_parent,
            parsed,
            &open_b,
            &open_p,
            threshold,
            Structure::Nothing,
        ) {
            bind(parsed, b, p, BindSource::DocumentLeftover);
        }
    }

    let base_ids: HashSet<&EntityUri> = base.iter().map(|b| &b.id).collect();
    let mut seen = HashSet::new();
    for (j, p) in parsed.iter().enumerate() {
        let Some(id) = &p.id else { continue };
        assert!(
            seen.insert(id.clone()),
            "id {id} bound to two parsed headlines"
        );
        assert!(
            authored[j] || base_ids.contains(id),
            "transferred id {id} is not in the base snapshot"
        );
    }
    sources
}

fn base_parents(base: &[BaseHeadline]) -> Vec<Option<EntityUri>> {
    (0..base.len())
        .map(|i| {
            (0..i)
                .rev()
                .find(|&k| base[k].depth < base[i].depth)
                .map(|k| base[k].id.clone())
        })
        .collect()
}

/// `Some(parent id)` once the parent is bound, `Some(None)` at the root, and
/// `None` while the parent is still unbound.
fn parsed_parent(parsed: &[ParsedHeadline], j: usize) -> Option<Option<EntityUri>> {
    match (0..j).rev().find(|&k| parsed[k].depth < parsed[j].depth) {
        None => Some(None),
        Some(k) => parsed[k].id.clone().map(Some),
    }
}

fn fits(
    structure: Structure,
    base: &[BaseHeadline],
    base_parent: &[Option<EntityUri>],
    parsed: &[ParsedHeadline],
    b: usize,
    p: usize,
) -> bool {
    match structure {
        Structure::Nothing => true,
        Structure::Depth => base[b].depth == parsed[p].depth,
        Structure::DepthAndParent => {
            base[b].depth == parsed[p].depth
                && parsed_parent(parsed, p).as_ref() == Some(&base_parent[b])
        }
    }
}

fn pair_similar(
    base: &[BaseHeadline],
    base_parent: &[Option<EntityUri>],
    parsed: &mut [ParsedHeadline],
    hb: &[usize],
    hp: &[usize],
    threshold: f64,
    structure: Structure,
) -> Vec<(usize, usize)> {
    let mut pairs = Vec::new();
    let mut open_b: Vec<usize> = hb.to_vec();
    let mut open_p: Vec<usize> = hp.to_vec();
    // Rounds, because a child can only agree on its parent once that parent paired.
    loop {
        let mut score: HashMap<(usize, usize), f64> = HashMap::new();
        for &p in &open_p {
            for &b in &open_b {
                if fits(structure, base, base_parent, parsed, b, p) {
                    let s = bigram_dice(&base[b].text, &parsed[p].text);
                    if s >= threshold {
                        score.insert((b, p), s);
                    }
                }
            }
        }
        let best_of =
            |key: &dyn Fn(&(usize, usize)) -> usize, x: usize| -> Option<(usize, usize)> {
                let mut best: Option<((usize, usize), f64)> = None;
                let mut tied = false;
                for (&pair, &s) in score.iter().filter(|(k, _)| key(k) == x) {
                    match best {
                        Some((_, bs)) if s < bs => {}
                        Some((_, bs)) if s == bs => tied = true,
                        _ => {
                            best = Some((pair, s));
                            tied = false;
                        }
                    }
                }
                if tied {
                    None
                } else {
                    best.map(|(pair, _)| pair)
                }
            };
        let round: Vec<(usize, usize)> = open_p
            .iter()
            .filter_map(|&p| best_of(&|k| k.1, p))
            .filter(|&(b, p)| best_of(&|k| k.0, b) == Some((b, p)))
            .collect();
        if round.is_empty() {
            return pairs;
        }
        for &(b, p) in &round {
            parsed[p].id = Some(base[b].id.clone());
            open_b.retain(|&x| x != b);
            open_p.retain(|&x| x != p);
        }
        pairs.extend(round);
    }
}

/// Sørensen–Dice coefficient over lower-cased character bigrams.
pub fn bigram_dice(a: &str, b: &str) -> f64 {
    let grams = |s: &str| -> Vec<(char, char)> {
        let cs: Vec<char> = s.to_lowercase().chars().collect();
        cs.windows(2).map(|w| (w[0], w[1])).collect()
    };
    let (ga, gb) = (grams(a), grams(b));
    if ga.is_empty() || gb.is_empty() {
        return if a.eq_ignore_ascii_case(b) { 1.0 } else { 0.0 };
    }
    let mut pool: HashMap<(char, char), usize> = HashMap::new();
    for g in &gb {
        *pool.entry(*g).or_default() += 1;
    }
    let mut common = 0usize;
    for g in &ga {
        if let Some(n) = pool.get_mut(g).filter(|n| **n > 0) {
            *n -= 1;
            common += 1;
        }
    }
    2.0 * common as f64 / (ga.len() + gb.len()) as f64
}

/// Index pairs `(i, j)` of a longest common subsequence of `a` and `b`, in
/// order.
fn lcs(a: &[usize], b: &[usize], eq: impl Fn(usize, usize) -> bool) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut len = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            len[i][j] = if eq(a[i], b[j]) {
                len[i + 1][j + 1] + 1
            } else {
                len[i + 1][j].max(len[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < n && j < m {
        if eq(a[i], b[j]) {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if len[i + 1][j] >= len[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

#[cfg(test)]
#[path = "identity_transfer_tests.rs"]
mod tests;
