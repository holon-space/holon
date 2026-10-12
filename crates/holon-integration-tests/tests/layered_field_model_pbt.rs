//! Layered-field experiment (FOLD-scope.a, holon-work#170): the write and read
//! rule for `collapsed` declared `[base, device]`, checked against three
//! invariants and the fold postconditions over fold clicks, the integration
//! home gaining the field, base sync between two devices and explicit
//! overrides.
//!
//! `LAYERED_FIELD_RULE=plain` selects the later-layer-wins rule, which this
//! test rejects; the default is the ruled hybrid rule.
//! `LAYERED_FIELD_OVERRIDES=off` drops the override gesture from the draw, so
//! a failure can only come from fallback writes.
//!
//! @pbt kind slice
//! @pbt covers layered-field

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::LazyLock;

use holon_api::block::Block;
use holon_integration_tests::pbt::layered_field::Device;
use holon_integration_tests::pbt::layered_field::Intent;
use holon_integration_tests::pbt::layered_field::LayeredField;
use holon_integration_tests::pbt::layered_field::Rule;
use holon_integration_tests::pbt::layered_field::Target;
use holon_integration_tests::pbt::layered_field::parse_own_page;
use holon_integration_tests::pbt::layered_field::seed_own_page_file;
use proptest::prelude::*;
use proptest::sample::select;
use proptest::test_runner::Config;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

/// One device's layer for one target: (value, intent), or unset.
type Layer = Option<(bool, Intent)>;
/// The newest write to a target: (time, writer), or none.
type Stamp = Option<(u64, Device)>;

#[derive(Debug, Clone)]
enum LayerOp {
    Fold(Device, Target),
    Override(Device, Target, bool),
    HomeGainsField,
    Sync,
}

fn override_weight() -> u32 {
    match std::env::var("LAYERED_FIELD_OVERRIDES").as_deref() {
        Err(std::env::VarError::NotPresent) | Ok("on") => 1,
        Ok("off") => 0,
        other => panic!("LAYERED_FIELD_OVERRIDES must be `on` or `off`, got {other:?}"),
    }
}

/// The reference predicts what every device displays, so the oracle never
/// takes a value from the SUT: the gestures seen so far, plus the ruled hybrid
/// rule applied to the reference's own flat (device, target) maps.
#[derive(Debug, Clone, Default)]
struct Gestures {
    gained: bool,
    overridden: BTreeSet<(Device, Target)>,
    base: BTreeMap<(Device, Target), (bool, (u64, Device))>,
    layer: BTreeMap<(Device, Target), (bool, Intent)>,
    clock: BTreeMap<Device, u64>,
    /// The value the `Fold` just applied must have written, so the fold arm can
    /// check the SUT's return value as well as its state.
    last_fold: Option<bool>,
}

impl Gestures {
    /// Which homes admit a base write, derived from the gestures alone: the own
    /// page always, the integration row only after `HomeGainsField`, the
    /// read-only `.cook` leaf never. The oracle must not ask the SUT this.
    fn base_admits(&self, target: Target) -> bool {
        match target {
            Target::OwnPage => true,
            Target::IntegrationRow => self.gained,
            Target::CookLeaf => false,
        }
    }

    /// The ruled hybrid read rule over the reference's own maps: an override
    /// wins, else base, else a live fallback, else expanded.
    fn displayed(&self, device: Device, target: Target) -> bool {
        let key = (device, target);
        match (self.layer.get(&key), self.base.get(&key)) {
            (Some((v, Intent::Override)), _) => *v,
            (_, Some((b, _))) => *b,
            (Some((v, Intent::Fallback)), None) => *v,
            (None, None) => false,
        }
    }

    /// The ruled hybrid write rule: an override absorbs the write, else base
    /// takes it if the home admits one, else it falls back to the device layer.
    fn write(&mut self, device: Device, target: Target, value: bool) {
        let key = (device, target);
        if matches!(self.layer.get(&key), Some((_, Intent::Override))) {
            self.layer.insert(key, (value, Intent::Override));
        } else if self.base_admits(target) {
            let clock = self.clock.entry(device).or_default();
            *clock += 1;
            let stamp = (*clock, device);
            self.base.insert(key, (value, stamp));
            self.layer.remove(&key);
        } else {
            self.layer.insert(key, (value, Intent::Fallback));
        }
    }

    /// The blocks `device`'s own page file must parse to: the seeded page with
    /// `collapsed` set to what base holds. Every other field of every block
    /// must survive the write-back untouched.
    fn expected_own_page(&self, device: Device) -> Vec<Block> {
        let collapsed = self
            .base
            .get(&(device, Target::OwnPage))
            .is_some_and(|held| held.0);
        let mut blocks = SEED_BLOCKS.clone();
        blocks
            .iter_mut()
            .find(|b| b.id == Target::OwnPage.uri())
            .expect("the seed holds the page heading")
            .collapsed = collapsed;
        blocks
    }

    /// Base converges on the latest stamp per target; the device layer and the
    /// `.cook` leaf's empty base stay untouched.
    fn sync(&mut self) {
        let mut merged: BTreeMap<Target, (bool, (u64, Device))> = BTreeMap::new();
        for ((_, target), held) in &self.base {
            let winner = merged.entry(*target).or_insert(*held);
            if held.1 > winner.1 {
                *winner = *held;
            }
        }
        let clock = self.clock.values().copied().max().unwrap_or(0);
        for device in Device::ALL {
            self.clock.insert(device, clock);
            for target in Target::ALL {
                match merged.get(&target) {
                    Some(held) => self.base.insert((device, target), *held),
                    None => self.base.remove(&(device, target)),
                };
            }
        }
    }
}

struct Catalog;

impl ReferenceStateMachine for Catalog {
    type State = Gestures;
    type Transition = LayerOp;

    fn init_state() -> BoxedStrategy<Gestures> {
        Just(Gestures::default()).boxed()
    }

    fn transitions(Gestures { gained, .. }: &Gestures) -> BoxedStrategy<LayerOp> {
        let device = || select(Device::ALL.to_vec());
        let target = || select(Target::ALL.to_vec());
        let common = prop_oneof![
            5 => (device(), target()).prop_map(|(d, t)| LayerOp::Fold(d, t)),
            override_weight() => (device(), target(), any::<bool>()).prop_map(|(d, t, v)| LayerOp::Override(d, t, v)),
            2 => Just(LayerOp::Sync),
        ];
        if *gained {
            return common.boxed();
        }
        prop_oneof![8 => common, 1 => Just(LayerOp::HomeGainsField)].boxed()
    }

    fn preconditions(state: &Gestures, op: &LayerOp) -> bool {
        !(state.gained && matches!(op, LayerOp::HomeGainsField))
    }

    fn apply(mut state: Gestures, op: &LayerOp) -> Gestures {
        state.last_fold = None;
        match op {
            LayerOp::HomeGainsField => state.gained = true,
            LayerOp::Override(device, target, value) => {
                state.overridden.insert((*device, *target));
                state
                    .layer
                    .insert((*device, *target), (*value, Intent::Override));
            }
            LayerOp::Fold(device, target) => {
                let value = !state.displayed(*device, *target);
                state.write(*device, *target, value);
                state.last_fold = Some(value);
            }
            LayerOp::Sync => state.sync(),
        }
        state
    }
}

/// Every other device's layer and base for `target`, so a gesture meant for
/// one device can be shown not to reach them.
fn snapshot_except(
    field: &LayeredField,
    device: Device,
    target: Target,
) -> Vec<(Device, Layer, Option<bool>)> {
    Device::ALL
        .into_iter()
        .filter(|d| *d != device)
        .map(|d| (d, field.layer(d, target), field.base(d, target)))
        .collect()
}

/// The seeded page, parsed once: the expectation differs from it only in
/// `collapsed`, and a parse per invariant check per device costs seconds.
static SEED_BLOCKS: LazyLock<Vec<Block>> = LazyLock::new(|| {
    let blocks = parse_own_page(&seed_own_page_file());
    let page = blocks
        .iter()
        .find(|b| b.id == Target::OwnPage.uri())
        .expect("the seed holds the page heading");
    let src = blocks
        .iter()
        .find(|b| b.id.to_string() == "block:layered-own-src")
        .expect("the seed holds the named source block");
    // A field left at its default cannot fail the equality below, so the fixture
    // must author every one of them away from it.
    assert!(page.tags.iter().any(|t| t == "home"), "{:?}", page.tags);
    assert!(page.properties.contains_key("AREA"));
    assert_eq!(page.requires.len(), 1);
    assert_eq!(page.advice_suppressed.len(), 1);
    assert_eq!(page.contributes_to.len(), 1);
    assert!(page.marks.is_some());
    assert!(page.widget_only);
    assert_eq!(src.content_type, holon_api::ContentType::Source);
    assert!(src.source_language.is_some(), "{:?}", src.source_language);
    assert_eq!(src.source_name.as_deref(), Some("own-page-query"));
    // `block_type` is the one compared field the org layer does not carry
    // (crates/holon-core/src/block_ordering.rs: only the typed slot speaks for
    // it, never a `:block_type:` property), so it is `None` on both sides and
    // the equality has no teeth on it. If a parse ever yields one, this fires
    // and the field must be authored like the others.
    assert!(
        blocks.iter().all(|b| b.block_type.is_none()),
        "the org layer now carries block_type: arm it in the fixture instead of excluding it"
    );
    // Sibling order is only observable where a parent has two children.
    let mut children_per_parent: BTreeMap<String, usize> = BTreeMap::new();
    for b in &blocks {
        *children_per_parent
            .entry(b.parent_id.to_string())
            .or_default() += 1;
    }
    assert!(
        children_per_parent.values().any(|n| *n >= 2),
        "the seed is a chain: no sibling order to compare, {children_per_parent:?}"
    );
    blocks
});

/// A block with the parse artifacts flattened away, so two parses of the same
/// page content compare equal: the two wall-clock stamps, and the `_`-prefixed
/// private carriers (`_drawer_order`), which the write-back adds for the
/// drawer keys it writes and which never reach the user's content.
fn stable_fields(mut block: Block) -> Block {
    block.created_at = 0;
    block.updated_at = 0;
    block.properties.retain(|key, _| !key.starts_with('_'));
    block
}

struct LayeredFieldTest;

impl StateMachineTest for LayeredFieldTest {
    type SystemUnderTest = LayeredField;
    type Reference = Catalog;

    fn init_test(_: &Gestures) -> LayeredField {
        let rule = match std::env::var("LAYERED_FIELD_RULE").as_deref() {
            Err(std::env::VarError::NotPresent) | Ok("hybrid") => Rule::Hybrid,
            Ok("plain") => Rule::Plain,
            other => panic!("LAYERED_FIELD_RULE must be `plain` or `hybrid`, got {other:?}"),
        };
        LayeredField::new(rule)
    }

    fn apply(mut field: LayeredField, gestures: &Gestures, op: LayerOp) -> LayeredField {
        match op {
            LayerOp::Fold(device, target) => {
                let base_before = field.base(device, target);
                let seen_stamp = field.base_stamp(device, target);
                let written = field.fold(device, target);
                let expected = gestures
                    .last_fold
                    .expect("the reference records every fold's value");
                assert_eq!(
                    written, expected,
                    "fold did not toggle: the chevron on {target:?} on {device:?} must flip \
                     the displayed {} to {expected}, wrote {written}",
                    !expected
                );
                assert_eq!(
                    field.effective(device, target),
                    written,
                    "invisible write: fold of {target:?} on {device:?} wrote {written} \
                     but the device still shows {}",
                    !written
                );
                if gestures.overridden.contains(&(device, target)) {
                    assert!(
                        matches!(field.layer(device, target), Some((_, Intent::Override)))
                            && field.base(device, target) == base_before,
                        "override lost: fold of {target:?} on {device:?} under an override \
                         must update the override and leave base alone; layer {:?}, \
                         base {base_before:?} -> {:?}",
                        field.layer(device, target),
                        field.base(device, target)
                    );
                } else if gestures.base_admits(target) {
                    assert_eq!(
                        field.base(device, target),
                        Some(written),
                        "fold missed base: fold of {target:?} on {device:?} without an \
                         override must write base, which offers a slot"
                    );
                    assert_eq!(
                        field.layer(device, target),
                        None,
                        "stale fallback: fold of {target:?} on {device:?} wrote base but left \
                         a value in the device layer"
                    );
                    assert!(
                        field.base_stamp(device, target) > seen_stamp,
                        "stale stamp: fold of {target:?} on {device:?} stamped {:?}, not later \
                         than the {seen_stamp:?} this device had already seen, so the next \
                         sync drops it",
                        field.base_stamp(device, target)
                    );
                } else {
                    assert_eq!(
                        field.layer(device, target),
                        Some((written, Intent::Fallback)),
                        "fallback lost: fold of {target:?} on {device:?}, whose home admits no \
                         base write, must leave {written} in the device layer tagged Fallback"
                    );
                    assert_eq!(
                        field.base(device, target),
                        base_before,
                        "wrote a closed home: fold of {target:?} on {device:?} moved base \
                         {base_before:?} -> {:?} although the home admits no base write",
                        field.base(device, target)
                    );
                }
            }
            LayerOp::Override(device, target, value) => {
                let base_before = field.base(device, target);
                let elsewhere = snapshot_except(&field, device, target);
                field.write_override(device, target, value);
                assert_eq!(
                    field.effective(device, target),
                    value,
                    "invisible write: override of {target:?} on {device:?} to {value}"
                );
                assert_eq!(
                    field.base(device, target),
                    base_before,
                    "override leaked to base: the \"only on this device\" override of \
                     {target:?} on {device:?} wrote base {base_before:?} -> {:?}, so the \
                     next sync carries it to the other device",
                    field.base(device, target)
                );
                for (other, layer, base) in elsewhere {
                    assert_eq!(
                        (field.layer(other, target), field.base(other, target)),
                        (layer, base),
                        "override leaked to {other:?}: the override of {target:?} on \
                         {device:?} changed layer {layer:?} -> {:?} / base {base:?} -> {:?} \
                         there",
                        field.layer(other, target),
                        field.base(other, target)
                    );
                }
            }
            LayerOp::HomeGainsField => {
                assert!(
                    !field.base_offers_slot(Target::IntegrationRow),
                    "home gained nothing: the integration home already admitted a base write \
                     before the gesture, so the gesture has no observable effect"
                );
                field.home_gains_field();
                assert!(
                    field.base_offers_slot(Target::IntegrationRow),
                    "home gained nothing: the gesture left the integration home refusing a \
                     base write"
                );
            }
            LayerOp::Sync => {
                let newest: Vec<(Target, Option<bool>, Stamp)> = Target::ALL
                    .into_iter()
                    .map(|target| {
                        let winner = Device::ALL
                            .into_iter()
                            .max_by_key(|d| field.base_stamp(*d, target))
                            .expect("at least one device");
                        (
                            target,
                            field.base(winner, target),
                            field.base_stamp(winner, target),
                        )
                    })
                    .collect();
                let layers_before: Vec<(Device, Target, Layer)> = Device::ALL
                    .into_iter()
                    .flat_map(|d| Target::ALL.map(move |t| (d, t)))
                    .map(|(d, t)| (d, t, field.layer(d, t)))
                    .collect();
                field.sync();
                for (target, value, stamp) in newest {
                    for device in Device::ALL {
                        assert_eq!(
                            (field.base(device, target), field.base_stamp(device, target)),
                            (value, stamp),
                            "sync did not propagate: {target:?} on {device:?} holds {:?} \
                             stamped {:?} after sync, not the newest replica {value:?} \
                             stamped {stamp:?}",
                            field.base(device, target),
                            field.base_stamp(device, target)
                        );
                    }
                }
                for (device, target, layer) in layers_before {
                    assert_eq!(
                        field.layer(device, target),
                        layer,
                        "sync touched the device layer: {target:?} on {device:?} was \
                         {layer:?}, is {:?}; the device layer never syncs",
                        field.layer(device, target)
                    );
                }
            }
        }
        field
    }

    fn check_invariants(field: &LayeredField, gestures: &Gestures) {
        for target in Target::ALL {
            assert_eq!(
                field.base_offers_slot(target),
                gestures.base_admits(target),
                "admission drift: the SUT's home of {target:?} disagrees with the gestures \
                 (home gained the field: {})",
                gestures.gained
            );
            for device in Device::ALL {
                let key = (device, target);
                assert_eq!(
                    field.effective(device, target),
                    gestures.displayed(device, target),
                    "display drift: {target:?} on {device:?}; reference layer {:?}, base {:?}",
                    gestures.layer.get(&key),
                    gestures.base.get(&key)
                );
                assert_eq!(
                    field.layer(device, target),
                    gestures.layer.get(&key).copied(),
                    "layer drift: {target:?} on {device:?}"
                );
                assert_eq!(
                    (field.base(device, target), field.base_stamp(device, target)),
                    (
                        gestures.base.get(&key).map(|held| held.0),
                        gestures.base.get(&key).map(|held| held.1)
                    ),
                    "base drift: {target:?} on {device:?}"
                );
            }
        }
        for device in Device::ALL {
            let parsed: Vec<Block> = parse_own_page(field.own_page_file(device))
                .into_iter()
                .map(stable_fields)
                .collect();
            let expected: Vec<Block> = gestures
                .expected_own_page(device)
                .into_iter()
                .map(stable_fields)
                .collect();
            assert_eq!(
                parsed,
                expected,
                "own page drift on {device:?}: the file does not parse back to the page \
                 the gestures predict, `collapsed` and every other field; file:\n{}",
                field.own_page_file(device)
            );
        }
        field.check_no_hidden_upstream();
    }
}

fn config() -> Config {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(16);
    Config {
        cases,
        max_shrink_iters: 200,
        failure_persistence: None,
        ..Config::default()
    }
}

prop_state_machine! {
    #![proptest_config(config())]

    #[test]
    fn layered_field_model_pbt(sequential 1..40 => LayeredFieldTest);
}
