//! The viewer contract is a wire format: it must survive a round trip, and
//! its one invariant must hold by construction.

use holon_net_layout::ArcKind;
use holon_net_layout::MarkingSnapshot;
use holon_net_layout::NetStructure;
use holon_net_layout::StepDelta;
use holon_net_layout::WhyNot;
use holon_net_layout::contract::ArcDecl;
use holon_net_layout::contract::Disclosure;
use holon_net_layout::contract::GuardDecl;
use holon_net_layout::contract::NetEvent;
use holon_net_layout::contract::NetVersion;
use holon_net_layout::contract::PlaceDecl;
use holon_net_layout::contract::PlaceMarking;
use holon_net_layout::contract::Provenance;
use holon_net_layout::contract::Reason;
use holon_net_layout::contract::SubnetDecl;
use holon_net_layout::contract::TokenView;
use holon_net_layout::contract::TransitionDecl;

fn version() -> NetVersion {
    NetVersion::new("blake3:0123456789abcdef")
}

fn round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("serialises");
    let back: T = serde_json::from_str(&json).expect("deserialises");
    assert_eq!(value, &back, "round trip changed the value: {json}");
}

#[test]
fn the_four_contract_types_round_trip() {
    round_trip(&NetStructure {
        version: version(),
        places: vec![PlaceDecl {
            id: "block.task_state".to_string(),
            label: "task_state".to_string(),
            token_type: Some("TaskState".to_string()),
            subnet: Some("block".to_string()),
            disclosure: Disclosure::Digest,
        }],
        transitions: vec![TransitionDecl {
            key: "op:block.set_field".to_string(),
            label: "block.set_field".to_string(),
            provenance: Provenance::Operation {
                entity: "block".to_string(),
                op: "set_field".to_string(),
            },
            guard: Some(GuardDecl {
                name: "is_task".to_string(),
                expr: Some("state != none".to_string()),
                opaque: false,
            }),
            subnet: Some("block".to_string()),
            analyzable: true,
        }],
        arcs: vec![ArcDecl {
            transition: "op:block.set_field".to_string(),
            place: "block.task_state".to_string(),
            kind: ArcKind::Produce,
            mode: 0,
        }],
        subnets: vec![SubnetDecl {
            id: "block".to_string(),
            label: "block".to_string(),
            members: vec!["block.task_state".to_string()],
        }],
    });

    round_trip(&MarkingSnapshot {
        version: version(),
        seq: 42,
        at_ms: 1_760_000_000_000,
        places: vec![PlaceMarking {
            place: "block.task_state".to_string(),
            count: 3,
            tokens: Some(vec![TokenView {
                uri: "block:abc".to_string(),
                version: 7,
                digest: "9f86d081".to_string(),
                summary: None,
            }]),
        }],
    });

    round_trip(&StepDelta {
        version: version(),
        seq: 43,
        at_ms: 1_760_000_000_001,
        events: vec![
            NetEvent::TokenTaken {
                place: "block.task_state".to_string(),
                uri: "block:abc".to_string(),
                version: 7,
            },
            NetEvent::StructureChanged {
                new_version: NetVersion::new("blake3:fedcba"),
            },
        ],
    });

    round_trip(&WhyNot::new(
        "op:block.set_field",
        "block:abc",
        43,
        vec![Reason::MissingInput {
            place: "block.task_state".to_string(),
        }],
    ));
}

/// "Fires" means exactly "no reason was collected" — the viewer can never
/// show a transition that is both enabled and refused.
#[test]
fn enabled_is_derived_from_the_collected_reasons() {
    assert!(WhyNot::new("op:block.set_field", "block:abc", 1, Vec::new()).enabled);
    assert!(
        !WhyNot::new(
            "op:block.set_field",
            "block:abc",
            1,
            vec![Reason::Unknown {
                why: "production residue".to_string()
            }]
        )
        .enabled
    );
}

/// An undeclared place shows a digest, never a value (backend report §6).
#[test]
fn an_undeclared_disclosure_defaults_to_digest() {
    let place: PlaceDecl = serde_json::from_str(
        r#"{"id":"block.body","label":"body","token_type":null,"subnet":null}"#,
    )
    .expect("disclosure is optional");
    assert_eq!(place.disclosure, Disclosure::Digest);
}
