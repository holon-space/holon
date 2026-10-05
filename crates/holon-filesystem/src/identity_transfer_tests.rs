use std::collections::HashSet;
use std::io::Write as _;

use proptest::prelude::*;

use super::*;

const VERBS: &[&str] = &[
    "Call", "Email", "Buy", "Fix", "Review", "Write", "Plan", "Read", "Clean", "Book",
];
const OBJECTS: &[&str] = &[
    "Anna",
    "the report",
    "milk",
    "the bike",
    "invoice 42",
    "slides",
    "the garden",
    "flights",
    "the parser",
    "notes",
];
const QUALIFIERS: &[&str] = &[
    "",
    "today",
    "before Friday",
    "for Martin",
    "again",
    "with Bob",
    "in the morning",
];
const NEW_ITEMS: &[&str] = &[
    "Water plants",
    "Renew passport",
    "Update CV",
    "Sort photos",
    "Pay rent",
    "Walk dog",
    "Defrost freezer",
    "Backup laptop",
    "Tune piano",
    "Order toner",
];
const EDIT_WORDS: &[&str] = &[
    "urgent", "now", "later", "maybe", "first", "done", "v2", "ASAP",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        assert!(n > 0);
        (self.next() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    InPlaceEdit,
    DeleteAdjacentEdit,
    TwoSimilar,
    EditReorder,
    RandomMix,
}

const SHAPES: &[Shape] = &[
    Shape::InPlaceEdit,
    Shape::DeleteAdjacentEdit,
    Shape::TwoSimilar,
    Shape::EditReorder,
    Shape::RandomMix,
];

/// One line of the edited file; `truth` is the base index the user's edit
/// descends from (`None` for a line the user typed new).
#[derive(Debug, Clone)]
struct Line {
    truth: Option<usize>,
    depth: usize,
    text: String,
}

#[derive(Debug)]
struct Case {
    base: Vec<BaseHeadline>,
    edited: Vec<Line>,
    /// Base indices the user deleted, edited or moved.
    in_play: HashSet<usize>,
}

fn headline(rng: &mut Rng) -> String {
    let q = rng.pick(QUALIFIERS);
    let s = format!("{} {}", rng.pick(VERBS), rng.pick(OBJECTS));
    if q.is_empty() { s } else { format!("{s} {q}") }
}

fn small_edit(rng: &mut Rng, text: &str) -> String {
    loop {
        let mut words: Vec<String> = text.split(' ').map(str::to_string).collect();
        match rng.below(4) {
            0 => words.push(rng.pick(EDIT_WORDS).to_string()),
            1 => {
                let i = rng.below(words.len());
                words[i] = rng.pick(EDIT_WORDS).to_string();
            }
            2 if words.len() >= 3 => {
                let i = rng.below(words.len());
                words.remove(i);
            }
            _ => {
                let i = rng.below(words.len());
                let mut cs: Vec<char> = words[i].chars().collect();
                let k = rng.below(cs.len());
                cs[k] = if cs[k] == 'x' { 'y' } else { 'x' };
                words[i] = cs.into_iter().collect();
            }
        }
        let out = words.join(" ");
        if out != text {
            return out;
        }
    }
}

fn base_tree(rng: &mut Rng, n: usize) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for _ in 0..n {
        let depth = match out.last() {
            None => 1,
            Some((d, _)) => 1 + rng.below(d + 1),
        };
        out.push((depth, headline(rng)));
    }
    out
}

fn gen_case(rng: &mut Rng, shape: Shape) -> Case {
    let n = 3 + rng.below(10);
    let mut tree = base_tree(rng, n);
    let similar_at = rng.below(tree.len() + 1);
    if shape == Shape::TwoSimilar {
        let at = similar_at;
        let depth = if at == 0 { 1 } else { tree[at - 1].0 };
        let stem = format!("{} {}", rng.pick(VERBS), rng.pick(OBJECTS));
        let q1 = rng.pick(&QUALIFIERS[1..]);
        let q2 = loop {
            let q = rng.pick(&QUALIFIERS[1..]);
            if q != q1 {
                break q;
            }
        };
        tree.insert(at, (depth, format!("{stem} {q2}")));
        tree.insert(at, (depth, format!("{stem} {q1}")));
    }
    let base: Vec<BaseHeadline> = tree
        .iter()
        .enumerate()
        .map(|(i, (depth, text))| BaseHeadline {
            id: EntityUri::block(&format!("b{i}")),
            depth: *depth,
            text: text.clone(),
        })
        .collect();
    let mut edited: Vec<Line> = base
        .iter()
        .enumerate()
        .map(|(i, b)| Line {
            truth: Some(i),
            depth: b.depth,
            text: b.text.clone(),
        })
        .collect();
    let mut in_play = HashSet::new();

    let edit = |edited: &mut Vec<Line>, in_play: &mut HashSet<usize>, rng: &mut Rng, i: usize| {
        edited[i].text = small_edit(rng, &edited[i].text);
        in_play.extend(edited[i].truth);
    };
    let delete = |edited: &mut Vec<Line>, in_play: &mut HashSet<usize>, i: usize| {
        in_play.extend(edited.remove(i).truth);
    };

    match shape {
        Shape::InPlaceEdit => {
            let i = rng.below(edited.len());
            edit(&mut edited, &mut in_play, rng, i);
        }
        Shape::DeleteAdjacentEdit => {
            let i = rng.below(edited.len() - 1);
            if rng.chance(50) {
                delete(&mut edited, &mut in_play, i);
                edit(&mut edited, &mut in_play, rng, i);
            } else {
                edit(&mut edited, &mut in_play, rng, i);
                delete(&mut edited, &mut in_play, i + 1);
            }
        }
        Shape::TwoSimilar => {
            let a = similar_at;
            let (x, y) = if rng.chance(50) {
                (a, a + 1)
            } else {
                (a + 1, a)
            };
            match rng.below(4) {
                0 => edit(&mut edited, &mut in_play, rng, x),
                1 => {
                    edit(&mut edited, &mut in_play, rng, x);
                    edit(&mut edited, &mut in_play, rng, y);
                }
                2 => {
                    edit(&mut edited, &mut in_play, rng, x);
                    delete(&mut edited, &mut in_play, y);
                }
                _ => {
                    let t = edited[x].text.clone();
                    let depth = edited[x].depth;
                    let words: Vec<&str> = t.split(' ').collect();
                    let twin = format!(
                        "{} {}",
                        words[..words.len() - 1].join(" "),
                        rng.pick(EDIT_WORDS)
                    );
                    edited.insert(
                        x.max(y),
                        Line {
                            truth: None,
                            depth,
                            text: twin,
                        },
                    );
                    edit(&mut edited, &mut in_play, rng, x.min(y));
                }
            }
        }
        Shape::EditReorder => {
            let i = rng.below(edited.len());
            edit(&mut edited, &mut in_play, rng, i);
            let from = if rng.chance(50) {
                i
            } else {
                rng.below(edited.len())
            };
            let line = edited.remove(from);
            in_play.extend(line.truth);
            let mut to = rng.below(edited.len() + 1);
            if to == from {
                to = (to + 1) % (edited.len() + 1);
            }
            edited.insert(to, line);
        }
        Shape::RandomMix => {
            for _ in 0..1 + rng.below(4) {
                if edited.is_empty() {
                    break;
                }
                let i = rng.below(edited.len());
                match rng.below(4) {
                    0 => edit(&mut edited, &mut in_play, rng, i),
                    1 => delete(&mut edited, &mut in_play, i),
                    2 => {
                        let depth = edited[i].depth;
                        let text = rng.pick(NEW_ITEMS).to_string();
                        edited.insert(
                            i,
                            Line {
                                truth: None,
                                depth,
                                text,
                            },
                        );
                    }
                    _ => {
                        let line = edited.remove(i);
                        in_play.extend(line.truth);
                        let to = rng.below(edited.len() + 1);
                        edited.insert(to, line);
                    }
                }
            }
        }
    }
    Case {
        base,
        edited,
        in_play,
    }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut prev = row[0];
        row[0] = i;
        for j in 1..=b.len() {
            let cur = row[j];
            row[j] = (row[j] + 1)
                .min(row[j - 1] + 1)
                .min(prev + usize::from(a[i - 1] != b[j - 1]));
            prev = cur;
        }
    }
    row[b.len()]
}

/// A person cannot tell which base line a new line descends from when another
/// base line whose identity is in play is at least as close in edit distance.
/// The judge uses Levenshtein so it is independent of the bigram metric the
/// similarity strategy pairs by.
fn ambiguous(case: &Case) -> bool {
    case.edited.iter().any(|l| match l.truth {
        Some(t) => {
            let own = levenshtein(&l.text, &case.base[t].text);
            case.in_play
                .iter()
                .filter(|&&u| u != t)
                .any(|&u| levenshtein(&l.text, &case.base[u].text) <= own)
        }
        None => case.in_play.iter().any(|&u| {
            let b = &case.base[u].text;
            levenshtein(&l.text, b) * 10 <= 4 * l.text.len().max(b.len())
        }),
    })
}

/// The store matcher's content-unique tier, run on what transfer left unbound.
fn bind_unique_text(base: &[BaseHeadline], parsed: &mut [ParsedHeadline]) -> Vec<usize> {
    let claimed: HashSet<EntityUri> = parsed.iter().filter_map(|p| p.id.clone()).collect();
    let open: Vec<usize> = (0..parsed.len())
        .filter(|&j| parsed[j].id.is_none())
        .collect();
    let mut bound = Vec::new();
    for &j in &open {
        let text = &parsed[j].text;
        if open.iter().filter(|&&k| &parsed[k].text == text).count() != 1 {
            continue;
        }
        let cands: Vec<&BaseHeadline> = base
            .iter()
            .filter(|b| &b.text == text && !claimed.contains(&b.id))
            .collect();
        if let [b] = cands[..] {
            parsed[j].id = Some(b.id.clone());
            bound.push(j);
        }
    }
    bound
}

fn recommended() -> TransferPolicy {
    sim(0.5, true, Structure::DepthAndParent, true)
}

#[derive(Debug, Default)]
struct Score {
    expected: usize,
    correct: usize,
    nonpair: usize,
    lost_untouched: usize,
    misbind_equal: usize,
    misbind_move: usize,
    misbind_hunk: usize,
    misbind_document: usize,
    misbind_unique: usize,
}

impl Score {
    fn misbind(&self) -> usize {
        self.misbind_equal
            + self.misbind_move
            + self.misbind_hunk
            + self.misbind_document
            + self.misbind_unique
    }
}

fn run(case: &Case, strategy: TransferPolicy) -> Score {
    let mut parsed: Vec<ParsedHeadline> = case
        .edited
        .iter()
        .map(|l| ParsedHeadline {
            id: None,
            depth: l.depth,
            text: l.text.clone(),
        })
        .collect();
    let sources = transfer_ids(&case.base, &mut parsed, strategy);
    let unique = bind_unique_text(&case.base, &mut parsed);
    let mut s = Score::default();
    for (j, l) in case.edited.iter().enumerate() {
        let want = l.truth.map(|t| case.base[t].id.clone());
        s.expected += usize::from(want.is_some());
        match (&want, &parsed[j].id) {
            (w, g) if w == g => s.correct += usize::from(w.is_some()),
            (Some(_), None) => {
                s.nonpair += 1;
                s.lost_untouched += usize::from(!case.in_play.contains(&l.truth.unwrap()));
            }
            _ if unique.contains(&j) => s.misbind_unique += 1,
            _ => match sources[j].expect("a bound id has a source") {
                BindSource::Equal => s.misbind_equal += 1,
                BindSource::ExactMove => s.misbind_move += 1,
                BindSource::Hunk => s.misbind_hunk += 1,
                BindSource::DocumentLeftover => s.misbind_document += 1,
            },
        }
    }
    s
}

fn sim(
    threshold: f64,
    exact_moves_first: bool,
    structure: Structure,
    document_leftovers: bool,
) -> TransferPolicy {
    TransferPolicy {
        exact_moves_first,
        pairing: PairStrategy::Similarity {
            threshold,
            structure,
            document_leftovers,
        },
    }
}

fn strategies() -> Vec<(&'static str, TransferPolicy)> {
    use Structure::Depth;
    use Structure::DepthAndParent;
    let positional = |exact_moves_first| TransferPolicy {
        exact_moves_first,
        pairing: PairStrategy::Positional,
    };
    vec![
        ("positional", positional(false)),
        ("positional+moves", positional(true)),
        ("similarity@0.5", sim(0.5, false, DepthAndParent, false)),
        (
            "similarity@0.3+moves",
            sim(0.3, true, DepthAndParent, false),
        ),
        (
            "similarity@0.5+moves",
            sim(0.5, true, DepthAndParent, false),
        ),
        (
            "similarity@0.7+moves",
            sim(0.7, true, DepthAndParent, false),
        ),
        ("similarity@0.5+moves-parent", sim(0.5, true, Depth, false)),
        (
            "similarity@0.3+moves+doc",
            sim(0.3, true, DepthAndParent, true),
        ),
        (
            "similarity@0.5+moves+doc",
            sim(0.5, true, DepthAndParent, true),
        ),
        (
            "similarity@0.7+moves+doc",
            sim(0.7, true, DepthAndParent, true),
        ),
    ]
}

/// Writes one TSV row per (case, strategy) to `$IDT_CORPUS_OUT`;
/// scripts/identity-transfer-rates.py aggregates it.
#[test]
#[ignore = "measurement corpus, run on demand"]
fn measure_identity_transfer_corpus() {
    let out = std::env::var("IDT_CORPUS_OUT").expect("IDT_CORPUS_OUT names the TSV to write");
    let mut f = std::io::BufWriter::new(std::fs::File::create(&out).expect("create corpus TSV"));
    writeln!(f, "shape\tseed\tstrategy\tambiguous\texpected\tcorrect\tnonpair\tlost_untouched\tmisbind_equal\tmisbind_move\tmisbind_hunk\tmisbind_document\tmisbind_unique").unwrap();
    for &shape in SHAPES {
        for seed in 0..5000u64 {
            let mut rng = Rng(seed ^ ((shape as u64) << 32));
            let case = gen_case(&mut rng, shape);
            let amb = ambiguous(&case);
            for (name, strategy) in strategies() {
                let s = run(&case, strategy);
                writeln!(
                    f,
                    "{shape:?}\t{seed}\t{name}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                    u8::from(amb),
                    s.expected,
                    s.correct,
                    s.nonpair,
                    s.lost_untouched,
                    s.misbind_equal,
                    s.misbind_move,
                    s.misbind_hunk,
                    s.misbind_document,
                    s.misbind_unique
                )
                .unwrap();
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// Every policy keeps the postconditions asserted inside `transfer_ids`.
    /// Under the recommended policy, on cases a person calls unambiguous, an
    /// in-place edit keeps its id and every untouched line keeps its id. The
    /// mis-bind rate of the other shapes is measured by
    /// `measure_identity_transfer_corpus`, not asserted per case.
    #[test]
    fn in_place_edits_and_untouched_lines_keep_their_ids(seed in any::<u64>(), shape in 0..SHAPES.len()) {
        let case = gen_case(&mut Rng(seed), SHAPES[shape]);
        for (_, strategy) in strategies() {
            run(&case, strategy);
        }
        prop_assume!(!ambiguous(&case));
        let s = run(&case, recommended());
        prop_assert_eq!(s.lost_untouched, 0, "untouched line lost its id: {:#?}", case);
        if SHAPES[shape] == Shape::InPlaceEdit {
            prop_assert_eq!(s.misbind(), 0, "in-place edit mis-bound: {:#?}", case);
        }
    }

    #[test]
    fn authored_ids_win(seed in any::<u64>()) {
        let case = gen_case(&mut Rng(seed), Shape::RandomMix);
        let parsed: Vec<ParsedHeadline> = case
            .edited
            .iter()
            .map(|l| ParsedHeadline { id: l.truth.map(|t| case.base[t].id.clone()), depth: l.depth, text: l.text.clone() })
            .collect();
        for (_, strategy) in strategies() {
            let mut p = parsed.clone();
            transfer_ids(&case.base, &mut p, strategy);
            for (got, want) in p.iter().zip(&parsed).filter(|(_, w)| w.id.is_some()) {
                prop_assert_eq!(&got.id, &want.id);
            }
        }
    }
}

/// Prints the first unambiguous cases where `$IDT_EXPLAIN_STRATEGY` mis-binds
/// or drops an untouched line, with the ids it chose.
#[test]
#[ignore = "diagnostic, run on demand"]
fn explain_identity_transfer_failures() {
    let want =
        std::env::var("IDT_EXPLAIN_STRATEGY").expect("IDT_EXPLAIN_STRATEGY names a strategy");
    let (_, strategy) = strategies()
        .into_iter()
        .find(|(n, _)| *n == want)
        .expect("known strategy");
    let mut shown = 0;
    for &shape in SHAPES {
        for seed in 0..5000u64 {
            let case = gen_case(&mut Rng(seed ^ ((shape as u64) << 32)), shape);
            if ambiguous(&case) {
                continue;
            }
            let s = run(&case, strategy);
            if s.misbind() + s.lost_untouched == 0 {
                continue;
            }
            let mut parsed: Vec<ParsedHeadline> = case
                .edited
                .iter()
                .map(|l| ParsedHeadline {
                    id: None,
                    depth: l.depth,
                    text: l.text.clone(),
                })
                .collect();
            transfer_ids(&case.base, &mut parsed, strategy);
            bind_unique_text(&case.base, &mut parsed);
            println!("=== {shape:?} seed {seed} {s:?}");
            for b in &case.base {
                println!("  base {} {} {:?}", b.id, "*".repeat(b.depth), b.text);
            }
            for (l, p) in case.edited.iter().zip(&parsed) {
                println!(
                    "  new  truth={:?} got={:?} {} {:?}",
                    l.truth,
                    p.id.as_ref().map(|i| i.to_string()),
                    "*".repeat(l.depth),
                    l.text
                );
            }
            shown += 1;
            if shown >= 12 {
                return;
            }
        }
    }
}
