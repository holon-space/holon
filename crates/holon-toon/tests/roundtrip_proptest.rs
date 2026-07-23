//! Round-trip PBT: `parse(render(forest)) == forest` over a generator that
//! covers every mapped construct — task states, priorities, tags, arbitrary
//! drawer props, `requires`/`advice` edges, scheduling, source blocks with
//! awkward multi-line bodies, image blocks — and adversarial content full of
//! the exact characters that clash with TOON's syntax (`: , " [ ] { } \` , tab,
//! newline, leading `-`/`#`).
//!
//! Generator idiom mirrors `crates/holon-org-format`'s proptests: small
//! `fn ... -> impl Strategy` builders composed into a recursive forest.

use holon_toon::models::BlockId;
use holon_toon::models::BlockNode;
use holon_toon::models::ContentType;
use holon_toon::models::Forest;
use holon_toon::models::Priority;
use holon_toon::models::TaskState;
use holon_toon::models::ToonBlock;
use holon_toon::parse;
use holon_toon::render;
use proptest::collection::btree_map;
use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;

/// Characters chosen to stress every quoting/escaping path.
const ADVERSARIAL: &[char] = &[
    'a', 'Z', '0', '9', ' ', '\t', '\n', ':', ',', '"', '\\', '[', ']', '{', '}', '=', '-', '#',
    '/', '.', 'é', '界',
];

fn adversarial_string(max: usize) -> impl Strategy<Value = String> {
    vec(prop::sample::select(ADVERSARIAL), 0..max).prop_map(|cs| cs.into_iter().collect())
}

/// Non-empty adversarial string (for bodies/paths, where empty means "absent").
fn nonempty_adversarial(max: usize) -> impl Strategy<Value = String> {
    vec(prop::sample::select(ADVERSARIAL), 1..max).prop_map(|cs| cs.into_iter().collect())
}

/// Bare block id: non-empty, whitespace-free, comma-free (comma is the id-list
/// separator). Includes `:` to force cell quoting.
fn block_id() -> impl Strategy<Value = BlockId> {
    let alphabet: Vec<char> = "abcdefABCDEF0123456789-_:".chars().collect();
    vec(prop::sample::select(alphabet), 1..16)
        .prop_map(|cs| BlockId::new(cs.into_iter().collect::<String>()).unwrap())
}

fn task_state() -> impl Strategy<Value = Option<TaskState>> {
    let states = vec!["TODO", "DOING", "DONE", "CANCELLED", "LATER", "NOW", "WAIT"];
    option::of(prop::sample::select(states).prop_map(|s| TaskState::new(s).unwrap()))
}

fn priority() -> impl Strategy<Value = Option<Priority>> {
    option::of(prop::sample::select(vec![
        Priority::A,
        Priority::B,
        Priority::C,
    ]))
}

/// Arbitrary drawer key: lowercase-ish, never colliding with the reserved
/// uppercase props keys (documented schema constraint).
fn prop_key() -> impl Strategy<Value = String> {
    let alphabet: Vec<char> = "abcdefghijklmnopqrstuvwxyz0123456789-_".chars().collect();
    vec(prop::sample::select(alphabet), 1..12).prop_map(|cs| cs.into_iter().collect())
}

fn tags() -> impl Strategy<Value = Vec<String>> {
    vec(nonempty_adversarial(6), 0..3)
}

fn id_list() -> impl Strategy<Value = Vec<BlockId>> {
    vec(block_id(), 0..3)
}

fn text_block() -> impl Strategy<Value = ToonBlock> {
    (
        block_id(),
        task_state(),
        priority(),
        tags(),
        adversarial_string(20),               // title (may be empty)
        option::of(nonempty_adversarial(24)), // body
        option::of(adversarial_string(12)),   // scheduled
        option::of(adversarial_string(12)),   // deadline
        id_list(),                            // requires
        id_list(),                            // advice_suppressed
        any::<bool>(),                        // collapsed
        btree_map(prop_key(), adversarial_string(16), 0..3), // props
    )
        .prop_map(
            |(
                id,
                state,
                priority,
                tags,
                title,
                body,
                scheduled,
                deadline,
                requires,
                advice_suppressed,
                collapsed,
                properties,
            )| {
                let mut b = ToonBlock::text(id, title);
                b.state = state;
                b.priority = priority;
                b.tags = tags;
                b.body = body;
                b.scheduled = scheduled;
                b.deadline = deadline;
                b.requires = requires;
                b.advice_suppressed = advice_suppressed;
                b.collapsed = collapsed;
                b.properties = properties;
                b
            },
        )
}

fn source_block() -> impl Strategy<Value = ToonBlock> {
    (
        block_id(),
        tags(),
        prop_key(),                           // language
        option::of(prop_key()),               // name
        option::of(nonempty_adversarial(40)), // code body (multi-line, specials)
        btree_map(prop_key(), adversarial_string(16), 0..2),
    )
        .prop_map(|(id, tags, lang, name, body, properties)| {
            let mut b = ToonBlock::text(id, String::new());
            b.content_type = ContentType::Source;
            b.tags = tags;
            b.source_language = Some(lang);
            b.source_name = name;
            b.body = body;
            b.properties = properties;
            b
        })
}

fn image_block() -> impl Strategy<Value = ToonBlock> {
    (block_id(), tags(), nonempty_adversarial(24)).prop_map(|(id, tags, path)| {
        let mut b = ToonBlock::text(id, String::new());
        b.content_type = ContentType::Image;
        b.tags = tags;
        b.content_path = Some(path);
        b
    })
}

fn any_block() -> impl Strategy<Value = ToonBlock> {
    prop_oneof![
        6 => text_block(),
        2 => source_block(),
        1 => image_block(),
    ]
}

fn forest_strategy() -> impl Strategy<Value = Forest> {
    let leaf = any_block().prop_map(BlockNode::leaf);
    let node = leaf.prop_recursive(4, 40, 4, |inner| {
        (any_block(), vec(inner, 0..4))
            .prop_map(|(b, children)| BlockNode::with_children(b, children))
    });
    vec(node, 1..5).prop_map(Forest::new)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn parse_render_is_identity(forest in forest_strategy()) {
        let rendered = render(&forest);
        let parsed = parse(&rendered).unwrap_or_else(|e| {
            panic!("parse failed on:\n{}\nerror: {}", rendered, e)
        });
        prop_assert_eq!(parsed, forest, "round-trip mismatch\nrendered:\n{}", rendered);
    }

    /// The rendered document must always parse (no panics / errors) — a weaker
    /// property that localizes parser robustness separately from equality.
    #[test]
    fn render_always_parses(forest in forest_strategy()) {
        let rendered = render(&forest);
        prop_assert!(parse(&rendered).is_ok(), "parse errored on:\n{}", rendered);
    }
}
