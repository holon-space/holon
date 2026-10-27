//! Perspectives / layouts as data (Vision Gap C8).
//!
//! A **perspective** is a named UI mode expressed entirely as blocks: a
//! container block whose children are *panel* declarations. Each panel declares
//! a data-source query (reusing the existing query-source-child mechanism),
//! optionally a render expression, and layout hints (region, width, collapse).
//! The perspective block itself carries the profile override and concealment
//! parameters. This is the exact same block shape the bundled
//! `block:root-layout` already uses (`assets/default/index.org`): the default
//! layout *is* a perspective, so there is no bespoke layout path to
//! special-case — a named perspective and the default layout are parsed by the
//! same code here.
//!
//! # Parse, don't validate
//!
//! [`PerspectiveSpec::parse`] converts a raw block subtree into a typed spec at
//! the boundary. Perspective-level declaration fields are namespaced with the
//! `perspective_` prefix; an unrecognized `perspective_*` key, or a recognized
//! key whose value is the wrong shape, is a **loud parse error** (never a
//! silently-dropped field). Non-namespaced properties (generic block metadata
//! like `sort_key`) are ignored, so a perspective block is still an ordinary
//! block.
//!
//! # Switching perspectives
//!
//! The active perspective is a pointer stored as the
//! [`ACTIVE_PERSPECTIVE_PROPERTY`] property on the well-known root-layout block
//! — a plain block property, so it persists through Loro exactly like collapse
//! state and survives restart. [`set_active_perspective`] writes it;
//! [`resolve_active_perspective`] follows it (defaulting to the root-layout
//! block itself when unset) and returns the active [`PerspectiveSpec`].
//!
//! # Slot resolution (RULING 2026-07-13)
//!
//! The ROOT display slot's content is resolved by an **ordinary query over
//! ordinary data** — the same mechanism as the main panel resolving from
//! `navigation.focus`. Both render-derivation arms
//! (`BlockDomain::render_entity` on the Turso arm,
//! `loro_ui_watcher::derive_render_expr` on the no-Turso arm)
//! call [`resolve_active_perspective`] — the pointer property is the degenerate
//! slot query — and render the resolved perspective's panels via
//! [`PerspectiveSpec::layout_expr`]. Switching = an ordinary `set_field` on the
//! pointer data; there is deliberately **no** `activate_perspective` op, which
//! makes the active layout rule-drivable for free.
//! See `docs/Proposals/PerspectivesAsData-C8.md`.

use crate::Value;
use crate::block::Block;
use crate::entity_uri::EntityUri;
use crate::render_types::RenderExpr;
use crate::types::EntityName;
use crate::types::QueryLanguage;
use crate::types::Region;
use crate::types::SourceLanguage;

/// The property on the well-known root-layout block that points at the active
/// perspective block. Absent → the root-layout block is itself the active
/// perspective (the default layout).
pub const ACTIVE_PERSPECTIVE_PROPERTY: &str = "active_perspective";

/// The mobile action bar's two tiers, narrowest first — `TargetScope`'s
/// `Block < Page < Global` order made structural, so what a user reaches with
/// their thumb first are the ops acting on the block they are editing.
///
/// The entity tier re-targets on every focus change (`chain_ops` projects the
/// focused block); the global tier does not depend on focus, so the bar keeps
/// offering navigation even with nothing focused.
pub const ACTION_BAR_ENTITY_OPS: &str = "list(#{gap: 8, horizontal: true, collection: chain_ops(0), \
                                     item_template: op_button(col(\"name\"))})";
/// App-level ops reached through the `navigation` entity — history and home,
/// which a phone has no keyboard chord for.
pub const ACTION_BAR_GLOBAL_OPS: &str = "list(#{gap: 8, horizontal: true, collection: ops_of(\"navigation:main\", #{surface: \
     \"action_bar\"}), item_template: op_button(col(\"name\"))})";

// Recognized `perspective_`-namespaced declaration fields on a perspective
// block.
const P_NAME: &str = "perspective_name";
const P_PROFILE: &str = "perspective_profile";
const P_CONCEAL_COMPLETED: &str = "perspective_conceal_completed";
const P_CONCEAL_TAGS: &str = "perspective_conceal_tags";
const P_ACTION_BAR: &str = "perspective_action_bar";

const RECOGNIZED_PERSPECTIVE_FIELDS: &[&str] = &[
    P_NAME,
    P_PROFILE,
    P_CONCEAL_COMPLETED,
    P_CONCEAL_TAGS,
    P_ACTION_BAR,
];

/// A panel's data source: a query in one of the supported languages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelSource {
    pub language: QueryLanguage,
    pub query: String,
}

/// One panel of a perspective: a data source plus layout hints. Parsed from a
/// child block of the perspective, whose own children carry the query-source
/// and (optional) render-source blocks — the same shape the bundled layout
/// uses.
#[derive(Debug, Clone, PartialEq)]
pub struct PanelSpec {
    /// The panel block's id.
    pub id: EntityUri,
    /// UI region this panel targets, when declared (`region: main`).
    pub region: Option<Region>,
    /// Ordering key among sibling panels (from the `sequence` property, else
    /// the child's positional index).
    pub sequence: i64,
    /// The panel's data-source query (a query-source child block). `None` for a
    /// panel that declares only a render (e.g. a static header).
    pub source: Option<PanelSource>,
    /// Raw render-DSL text (a `render`-source child block), when declared.
    pub render: Option<String>,
    /// Preferred width hint (`ideal_width` property).
    pub ideal_width: Option<f64>,
    /// Collapse target (`collapse_to` property, e.g. `drawer`).
    pub collapse_to: Option<String>,
    /// Column priority for responsive drop order (`column_priority` property).
    pub column_priority: Option<i64>,
}

/// Concealment parameters for a perspective — declarative "hide these" hints
/// the render layer applies. Deliberately small in v1; extend as the render
/// seam grows to consume more.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ConcealmentParams {
    /// Hide blocks whose task state is a completed/done state.
    pub hide_completed: bool,
    /// Hide blocks carrying any of these tags.
    pub hide_tags: Vec<String>,
}

/// A named UI mode as data: the panels it shows, a profile override, and
/// concealment parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct PerspectiveSpec {
    /// The perspective (container) block's id.
    pub id: EntityUri,
    /// Human-readable name (`perspective_name` property, else the block
    /// content).
    pub name: String,
    /// The panels, ordered by `(sequence, id)`.
    pub panels: Vec<PanelSpec>,
    /// Profile this perspective resolves collection variants through, if any
    /// (`perspective_profile` property). Named after the profile-cache key —
    /// the `entity_name` a profile block declares — so a "Kanban perspective"
    /// points at a profile whose collection variants default to `board`. When
    /// set, panels of the active perspective resolve
    /// `resolve_collection_variants_named(profile_override)` instead of the
    /// default `collection` profile.
    pub profile_override: Option<EntityName>,
    /// Concealment parameters.
    pub concealment: ConcealmentParams,
    /// Whether the narrow layout docks the action bar
    /// (`perspective_action_bar`, default `true`). Its global tier offers
    /// `navigation:main` ops, so a perspective for a session without that
    /// entity turns it off.
    pub action_bar: bool,
}

impl PerspectiveSpec {
    /// Parse the perspective rooted at `perspective_id` out of `blocks`.
    ///
    /// `blocks` is any collection that contains the perspective block, its
    /// panel children, and those panels' source children (e.g. a whole
    /// document's block set). Fails loud when the perspective block is
    /// missing, when a `perspective_*` field is unrecognized or malformed,
    /// or when a panel's layout hint has the wrong value shape.
    pub fn parse(perspective_id: &EntityUri, blocks: &[Block]) -> anyhow::Result<Self> {
        let block = blocks
            .iter()
            .find(|b| &b.id == perspective_id)
            .ok_or_else(|| {
                anyhow::anyhow!("perspective block {perspective_id} not found in block set")
            })?;

        // Fail loud on any unrecognized `perspective_`-namespaced field.
        for key in block.properties.keys() {
            if key.starts_with("perspective_")
                && !RECOGNIZED_PERSPECTIVE_FIELDS.contains(&key.as_str())
            {
                anyhow::bail!(
                    "perspective {perspective_id}: unknown perspective field {key:?} (recognized: \
                     {RECOGNIZED_PERSPECTIVE_FIELDS:?})"
                );
            }
        }

        let name = block
            .get_property_str(P_NAME)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| block.content.clone());

        let profile_override =
            match block.get_property_str(P_PROFILE) {
                Some(raw) => Some(EntityName::parse_named(&raw).map_err(|e| {
                    anyhow::anyhow!("perspective {perspective_id}: {P_PROFILE}: {e}")
                })?),
                None => None,
            };

        let hide_completed = match block.properties.get(P_CONCEAL_COMPLETED) {
            Some(v) => parse_bool(v).ok_or_else(|| {
                anyhow::anyhow!(
                    "perspective {perspective_id}: {P_CONCEAL_COMPLETED} must be a boolean, got \
                     {v:?}"
                )
            })?,
            None => false,
        };
        let action_bar = match block.properties.get(P_ACTION_BAR) {
            Some(v) => parse_bool(v).ok_or_else(|| {
                anyhow::anyhow!(
                    "perspective {perspective_id}: {P_ACTION_BAR} must be a boolean, got {v:?}"
                )
            })?,
            None => true,
        };
        let hide_tags = block
            .get_property_str(P_CONCEAL_TAGS)
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let mut panels: Vec<PanelSpec> = blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| &b.parent_id == perspective_id)
            .map(|(idx, panel)| PanelSpec::parse(panel, blocks, idx as i64))
            .collect::<anyhow::Result<_>>()?;
        panels.sort_by(|a, b| {
            a.sequence
                .cmp(&b.sequence)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });

        Ok(PerspectiveSpec {
            id: perspective_id.clone(),
            name,
            panels,
            profile_override,
            concealment: ConcealmentParams {
                hide_completed,
                hide_tags,
            },
            action_bar,
        })
    }

    /// Synthesize the responsive multi-panel layout `RenderExpr` for this
    /// perspective — the render both derivation arms return for the ROOT
    /// display slot. This replaces the formerly-hardcoded `root_layout`
    /// variant in `block_profile.yaml`: the layout is now derived from panel
    /// DATA (`collapse_to`, panel order), so any perspective block the slot
    /// query resolves gets the same treatment as the bundled default.
    ///
    /// Shape (identical to the retired YAML variant when applied to the
    /// bundled `block:root-layout` panels):
    /// - ≤600 px: panels stacked over a bottom-docked operation bar; drawer
    ///   panels open as overlays.
    /// - ≤1000 px: drawer panels *before* the first plain panel shrink
    ///   (persistent nav), drawer panels after it overlay (contextual).
    /// - wide: all drawer panels shrink.
    ///
    /// Fails loud when the perspective has no displayable panel.
    pub fn layout_expr(&self) -> anyhow::Result<RenderExpr> {
        let dsl = self.layout_dsl()?;
        crate::render_dsl::parse_render_dsl(&dsl).map_err(|e| {
            anyhow::anyhow!(
                "perspective {}: synthesized layout DSL failed to parse: {e}\nDSL: {dsl}",
                self.id
            )
        })
    }

    pub fn has_displayable_panel(&self) -> bool {
        self.panels.iter().any(PanelSpec::is_displayable)
    }

    /// The DSL text behind [`Self::layout_expr`] — split out so tests can pin
    /// the synthesized shape against the previously-hardcoded YAML variant.
    pub fn layout_dsl(&self) -> anyhow::Result<String> {
        let panels: Vec<&PanelSpec> = self.panels.iter().filter(|p| p.is_displayable()).collect();
        if panels.is_empty() {
            anyhow::bail!(
                "perspective {} ({:?}) has no displayable panels — every panel lacks both a \
                 query-source and a render child, so there is nothing to lay out",
                self.id,
                self.name
            );
        }
        let first_plain = panels
            .iter()
            .position(|p| p.collapse_to.as_deref() != Some("drawer"));

        #[derive(Clone, Copy)]
        enum Breakpoint {
            Narrow,
            Mid,
            Wide,
        }
        let cell = |p: &PanelSpec, idx: usize, bp: Breakpoint| -> String {
            if p.collapse_to.as_deref() == Some("drawer") {
                let mode = match bp {
                    Breakpoint::Narrow => "overlay",
                    Breakpoint::Wide => "shrink",
                    Breakpoint::Mid => {
                        if first_plain.is_some_and(|fp| idx < fp) {
                            "shrink"
                        } else {
                            "overlay"
                        }
                    }
                };
                format!(
                    "drawer(\"{id}\", live_block(\"{id}\"), #{{mode: \"{mode}\"}})",
                    id = p.id
                )
            } else {
                format!("live_block(\"{id}\")", id = p.id)
            }
        };
        let cols = |bp: Breakpoint| {
            panels
                .iter()
                .enumerate()
                .map(|(i, p)| cell(p, i, bp))
                .collect::<Vec<_>>()
                .join(", ")
        };

        let narrow = if self.action_bar {
            format!(
                "bottom_dock(\n    columns({}),\n    row(#{{gap: 8}}, {ACTION_BAR_ENTITY_OPS}, \
                 {ACTION_BAR_GLOBAL_OPS}))",
                cols(Breakpoint::Narrow)
            )
        } else {
            format!("columns({})", cols(Breakpoint::Narrow))
        };
        Ok(format!(
            "if_space(600,\n  {narrow},\n  if_space(1000,\n    columns({mid}),\n    \
             columns({wide})))",
            mid = cols(Breakpoint::Mid),
            wide = cols(Breakpoint::Wide),
        ))
    }
}

impl PanelSpec {
    /// A panel takes part in the visual layout when it declares a data source
    /// or a render. Machinery-only children of a perspective block (e.g. the
    /// bundled advice-rule container, whose only child is a
    /// `holon_advice_rule_yaml` source) carry neither and are not columns.
    pub fn is_displayable(&self) -> bool {
        self.source.is_some() || self.render.is_some()
    }

    fn parse(panel: &Block, blocks: &[Block], positional: i64) -> anyhow::Result<Self> {
        let region =
            match panel.get_property_str("region") {
                Some(raw) => Some(raw.parse::<Region>().map_err(|e| {
                    anyhow::anyhow!("panel {}: invalid region {raw:?}: {e}", panel.id)
                })?),
                None => None,
            };

        let sequence = match panel.properties.get("sequence") {
            Some(v) => v.as_i64().ok_or_else(|| {
                anyhow::anyhow!("panel {}: sequence must be an integer, got {v:?}", panel.id)
            })?,
            None => positional,
        };

        let ideal_width = match panel.properties.get("ideal_width") {
            Some(v) => Some(v.as_f64().ok_or_else(|| {
                anyhow::anyhow!(
                    "panel {}: ideal_width must be a number, got {v:?}",
                    panel.id
                )
            })?),
            None => None,
        };
        let column_priority = match panel.properties.get("column_priority") {
            Some(v) => Some(v.as_i64().ok_or_else(|| {
                anyhow::anyhow!(
                    "panel {}: column_priority must be an integer, got {v:?}",
                    panel.id
                )
            })?),
            None => None,
        };
        let collapse_to = panel.get_property_str("collapse_to");

        // The panel's data source and render are declared by its child source
        // blocks — exactly the query-source / render-source children the bundled
        // layout panels carry.
        let mut source: Option<PanelSource> = None;
        let mut render: Option<String> = None;
        for child in blocks.iter().filter(|b| b.parent_id == panel.id) {
            match &child.source_language {
                Some(SourceLanguage::Query(language)) => {
                    if source.is_some() {
                        anyhow::bail!(
                            "panel {}: multiple query-source children; a panel declares one source",
                            panel.id
                        );
                    }
                    source = Some(PanelSource {
                        language: *language,
                        query: child.content.clone(),
                    });
                }
                Some(SourceLanguage::Render) => {
                    render = Some(child.content.clone());
                }
                _ => {}
            }
        }

        Ok(PanelSpec {
            id: panel.id.clone(),
            region,
            sequence,
            source,
            render,
            ideal_width,
            collapse_to,
            column_priority,
        })
    }
}

/// Read the active-perspective pointer off the root-layout block, returning the
/// pointed-to perspective id, or the root-layout id itself when unset (the
/// default layout is the default perspective).
pub fn active_perspective_id(
    root_layout_id: &EntityUri,
    blocks: &[Block],
) -> anyhow::Result<EntityUri> {
    let root = blocks
        .iter()
        .find(|b| &b.id == root_layout_id)
        .ok_or_else(|| anyhow::anyhow!("root-layout block {root_layout_id} not found"))?;
    match root.get_property_str(ACTIVE_PERSPECTIVE_PROPERTY) {
        Some(raw) => EntityUri::parse(&raw).map_err(|e| {
            anyhow::anyhow!(
                "root-layout {ACTIVE_PERSPECTIVE_PROPERTY} {raw:?} is not a valid id: {e}"
            )
        }),
        None => Ok(root_layout_id.clone()),
    }
}

/// Resolve and parse the currently-active perspective — the ROOT display
/// slot's resolution query in its degenerate (pointer-property) form. Both
/// render-derivation arms call this to pick the panels the root slot shows
/// (see module docs).
pub fn resolve_active_perspective(
    root_layout_id: &EntityUri,
    blocks: &[Block],
) -> anyhow::Result<PerspectiveSpec> {
    let active = active_perspective_id(root_layout_id, blocks)?;
    PerspectiveSpec::parse(&active, blocks)
}

/// Set the active-perspective pointer on the root-layout block. This is the
/// state mutation behind the `activate_perspective` operation: a plain block
/// property write, so it persists in Loro and survives restart like collapse
/// state.
pub fn set_active_perspective(root_layout: &mut Block, perspective_id: &EntityUri) {
    root_layout.set_property(
        ACTIVE_PERSPECTIVE_PROPERTY,
        perspective_id.as_str().to_string(),
    );
}

fn parse_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Boolean(b) => Some(*b),
        Value::String(s) => match s.as_str() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests;
