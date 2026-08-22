//! ONE minimal ten-axis profile yaml, shared by every test in this crate.
//!
//! Shared deliberately: two hand-maintained copies of a ten-axis fixture drift,
//! and a drifted fixture is how a test starts certifying something other than
//! what it claims.

/// A complete, deliberately UNREMARKABLE profile: every axis present, nothing
/// declared that a lossless format would contradict.
pub const MINIMAL: &str = r#"
profile: test
not_yet_certified:
  # No stub implements these probes; org drives them. Coverage is per-format,
  # and every marker states its reason — a bare marker is a load error.
  - clause: hosted_kinds
    reason: no stub in this crate drives it; the org harness does
  - clause: content_representation
    reason: no stub in this crate drives it; the org harness does
  - clause: content_inline_constructs
    reason: no stub in this crate drives it; the org harness does
  - clause: content_block_constructs
    reason: no stub in this crate drives it; the org harness does
  - clause: property_keys_charset
    reason: no stub in this crate drives it; the org harness does
  - clause: property_keys_collision
    reason: no stub in this crate drives it; the org harness does
  - clause: property_values_multi_value
    reason: no stub in this crate drives it; the org harness does
  - clause: property_values_reference_values
    reason: no stub in this crate drives it; the org harness does
  - clause: ordering_sibling_order
    reason: no stub in this crate drives it; the org harness does
  - clause: ordering_order_key_durable
    reason: no stub in this crate drives it; the org harness does
  - clause: ordering_property_order
    reason: no stub in this crate drives it; the org harness does
  - clause: hierarchy_shape
    reason: no stub in this crate drives it; the org harness does
  - clause: hierarchy_max_depth
    reason: no stub in this crate drives it; the org harness does
  - clause: hierarchy_cycles
    reason: no stub in this crate drives it; the org harness does
  - clause: identity_id_space
    reason: no stub in this crate drives it; the org harness does
  - clause: identity_id_origin
    reason: no stub in this crate drives it; the org harness does
  - clause: identity_id_constraints
    reason: no stub in this crate drives it; the org harness does
  - clause: identity_carriers
    reason: no stub in this crate drives it; the org harness does
  - clause: identity_carrier_disagreement
    reason: no stub in this crate drives it; the org harness does
  - clause: mutation_write_leg
    reason: no stub in this crate drives it; the org harness does
  - clause: mutation_unit_of_write
    reason: no stub in this crate drives it; the org harness does
  - clause: assets_attachments
    reason: no stub in this crate drives it; the org harness does
  - clause: assets_binary_inline
    reason: no stub in this crate drives it; the org harness does
  - clause: assets_extensions
    reason: no stub in this crate drives it; the org harness does
enforced_by:
  # The parse/render code of this crate — the only layer a format-crate
  # certification harness can reach, and therefore the only layer whose
  # clauses may be marked `not_yet_certified` here.
  format:
    - hosted_kinds
    - content_representation
    - content_inline_constructs
    - content_block_constructs
    - property_keys_charset
    - property_keys_case
    - property_keys_reserved_prefixes
    - property_keys_collision
    - property_keys_schema_required
    - property_values_types
    - property_values_empty_string
    - property_values_null
    - property_values_multi_value
    - property_values_reference_values
    - ordering_sibling_order
    - ordering_order_key_durable
    - ordering_property_order
    - hierarchy_shape
    - hierarchy_max_depth
    - hierarchy_cycles
    - identity_id_space
    - identity_id_origin
    - identity_id_constraints
    - identity_carriers
    - identity_carrier_disagreement
    - mutation_write_leg
    - mutation_unit_of_write
    - assets_attachments
    - assets_binary_inline
    - assets_extensions
  # The sync controller / consolidator. These are REAL rules that a
  # format-crate probe structurally cannot see; they are not TODOs here.
  # Each deferral NAMES the code that enforces it. A deferral without a site is
  # indistinguishable from a clause nobody looked at, so the loader refuses one.
  sync:
    # MEASURED, and it CORRECTS my earlier citation: the refusal is real but it
    # is not where I said. `name_chain` bails when an ANCESTOR of a page is
    # absent from the page store — i.e. a page whose ancestor is not a page —
    # which IS "a :Page: under a non-page is refused", at resolution time
    # rather than at the moment of the edit.
    - clause: hierarchy_constraints
      site: "crates/holon-filesystem/src/sync_ports.rs:277-285 (name_chain ancestor-miss bail)"
    # A rename is a filesystem event; the controller derives the new path from
    # the new title and retires the old one.
    - clause: identity_rename_stability
      site: "crates/holon-filesystem/src/file_sync_controller.rs:573-580 (page-rename path derivation)"
    # Concurrency and merge belong to the consolidator (Model.md layer 2).
    - clause: ordering_concurrent_insert
      site: "crates/holon-loro/src/text_merge_provider.rs:126 (merge_text, 3-way merge path)"
    - clause: mutation_merge_granularity
      site: "crates/holon-loro/src/text_merge_provider.rs:126 (merge_text, 3-way merge path)"
    # Surfacing a conflict is a controller concern, above the format entirely.
    - clause: mutation_conflict_surface
      site: "crates/holon-filesystem/src/file_sync_controller.rs:1042 (3-way merger wiring, no-store conflict path)"
  # The OPERATION layer: `BlockOperations` refuses the edit BEFORE it reaches a
  # file — pre-flight, leaving the tree untouched.
  operation:
    - clause: hierarchy_reparent
      site: "crates/holon-core/src/traits.rs:2429-2440 (move_block page-under-non-page refusal; shared predicate block_op_catalog::page_under_non_page_prohibited; pinned by block_operations_tests.rs:1290, which asserts the tree is UNTOUCHED after the refusal)"
  # Refused when the TYPE is declared, not when a value is written.
  declaration:
    - clause: computed_live
      site: "crates/holon/src/core/type_declaration.rs:45-81 (declare_type)"
    - clause: computed_persisted
      site: "crates/holon/src/core/type_declaration.rs:45-81 (declare_type; CV-E lands in 2b.6)"
    - clause: computed_expression_closure
      site: "crates/holon/src/core/type_declaration.rs:45-81 (declare_type)"

fidelity_axes:
  hosted_kinds: [hierarchical]
  content:
    representation: opaque_text
    inline_constructs: []
    block_constructs: [paragraph]
  property_keys:
    charset: any
    case: sensitive
    reserved_prefixes: []
    reserved_keys: []
    collision: last_wins
    schema_required: open
  property_values:
    types: [string]
    empty_string: representable
    null: dropped
    multi_value:
      kind: none
    reference_values: none
  ordering:
    sibling_order: file_position
    order_key_durable: derived
    concurrent_insert: positional_conflict
    property_order: preserved
  hierarchy:
    shape: tree
    max_depth: unbounded
    reparent: free
    constraints: []
    cycles: rejected
  identity:
    id_space: opaque_string
    id_origin: authored
    id_constraints: []
    rename_stability: [file_rename, title_rename, move]
    carriers: [drawer_id]
    carrier_disagreement: error
  computed:
    computed_live: full
    computed_persisted:
      kind: string_only
    expression_closure: computation_plus_script
  mutation:
    write_leg: file
    unit_of_write: file
    merge_granularity: file
    conflict_surface: log
  assets:
    attachments: none
    binary_inline: none
    extensions: []
"#;

/// `MINIMAL` with one `old` substring swapped for `new` — the way every test
/// states the ONE clause it is about.
pub fn minimal_with(old: &str, new: &str) -> String {
    assert!(
        MINIMAL.contains(old),
        "fixture drift: {old:?} is no longer in the minimal profile"
    );
    MINIMAL.replace(old, new)
}
