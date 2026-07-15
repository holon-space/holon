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

use crate::block::Block;
use crate::entity_uri::EntityUri;
use crate::render_types::RenderExpr;
use crate::types::EntityName;
use crate::types::QueryLanguage;
use crate::types::Region;
use crate::types::SourceLanguage;
use crate::Value;

/// The property on the well-known root-layout block that points at the active
/// perspective block. Absent → the root-layout block is itself the active
/// perspective (the default layout).
pub const ACTIVE_PERSPECTIVE_PROPERTY: &str = "active_perspective";

// Recognized `perspective_`-namespaced declaration fields on a perspective
// block.
const P_NAME: &str = "perspective_name";
const P_PROFILE: &str = "perspective_profile";
const P_CONCEAL_COMPLETED: &str = "perspective_conceal_completed";
const P_CONCEAL_TAGS: &str = "perspective_conceal_tags";

const RECOGNIZED_PERSPECTIVE_FIELDS: &[&str] =
    &[P_NAME, P_PROFILE, P_CONCEAL_COMPLETED, P_CONCEAL_TAGS];

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

        let profile_override = match block.get_property_str(P_PROFILE) {
            Some(raw) => {
                // EntityName normalizes `_` → `-`; validate the normalized form is
                // a legal profile-cache key (URI-scheme shape) and fail loud.
                let normalized = raw.replace('_', "-");
                let mut chars = normalized.chars();
                let valid = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
                    && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
                if !valid {
                    anyhow::bail!(
                        "perspective {perspective_id}: {P_PROFILE} {raw:?} is not a valid profile \
                         name (must be alphanumeric with '-'/'_'/'.'/'+', starting with a letter)"
                    );
                }
                Some(EntityName::new(raw))
            }
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

        Ok(format!(
            "if_space(600,\n  bottom_dock(\n    columns({narrow}),\n    columns(#{{gap: 8, \
             collection: chain_ops(0), item_template: op_button(col(\"name\"))}})),\n  \
             if_space(1000,\n    columns({mid}),\n    columns({wide})))",
            narrow = cols(Breakpoint::Narrow),
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
mod tests {
    use super::*;
    use crate::block::Block;

    fn panel_block(id: &str, parent: &str) -> Block {
        Block::new_text(EntityUri::block(id), EntityUri::block(parent), id)
    }

    fn source_child(id: &str, parent: &str, language: QueryLanguage, query: &str) -> Block {
        let mut b = Block::new_text(EntityUri::block(id), EntityUri::block(parent), query);
        b.source_language = Some(SourceLanguage::Query(language));
        b
    }

    fn render_child(id: &str, parent: &str, dsl: &str) -> Block {
        let mut b = Block::new_text(EntityUri::block(id), EntityUri::block(parent), dsl);
        b.source_language = Some(SourceLanguage::Render);
        b
    }

    /// A perspective with two panels, each with its own query, parses into a
    /// typed spec whose panels carry the right languages and queries.
    #[test]
    fn parses_panels_with_queries() {
        let mut persp = panel_block("tasks-view", "root-layout");
        persp.set_property(P_NAME, "Tasks".to_string());
        persp.parent_id = EntityUri::no_parent();

        let mut left = panel_block("tv-left", "tasks-view");
        left.set_property("region", "left_sidebar".to_string());
        left.set_property("sequence", 0i64);
        let mut main = panel_block("tv-main", "tasks-view");
        main.set_property("region", "main".to_string());
        main.set_property("sequence", 1i64);

        let blocks = vec![
            persp,
            left,
            source_child(
                "tv-left::src",
                "tv-left",
                QueryLanguage::HolonSql,
                "SELECT * FROM block",
            ),
            main,
            source_child(
                "tv-main::src",
                "tv-main",
                QueryLanguage::HolonGql,
                "MATCH (b:block) RETURN b",
            ),
            render_child("tv-main::render", "tv-main", "tree(#{})"),
        ];

        let spec = PerspectiveSpec::parse(&EntityUri::block("tasks-view"), &blocks).unwrap();
        assert_eq!(spec.name, "Tasks");
        assert_eq!(spec.panels.len(), 2);
        assert_eq!(spec.panels[0].id, EntityUri::block("tv-left"));
        assert_eq!(spec.panels[0].region, Some(Region::LeftSidebar));
        assert_eq!(
            spec.panels[0].source,
            Some(PanelSource {
                language: QueryLanguage::HolonSql,
                query: "SELECT * FROM block".to_string()
            })
        );
        assert_eq!(spec.panels[1].id, EntityUri::block("tv-main"));
        assert_eq!(
            spec.panels[1].source.as_ref().unwrap().language,
            QueryLanguage::HolonGql
        );
        assert_eq!(spec.panels[1].render.as_deref(), Some("tree(#{})"));
    }

    /// Parse-don't-validate: an unrecognized `perspective_*` field is a loud
    /// error, not a silently-dropped property.
    #[test]
    fn unknown_perspective_field_fails_loud() {
        let mut persp = panel_block("p", "root-layout");
        persp.set_property("perspective_bogus", "x".to_string());
        let blocks = vec![persp];
        let err = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap_err();
        assert!(
            err.to_string().contains("unknown perspective field"),
            "got: {err}"
        );
    }

    /// A malformed recognized field also fails loud.
    #[test]
    fn malformed_conceal_completed_fails_loud() {
        let mut persp = panel_block("p", "root-layout");
        persp.set_property(P_CONCEAL_COMPLETED, "maybe".to_string());
        let blocks = vec![persp];
        let err = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap_err();
        assert!(err.to_string().contains("must be a boolean"), "got: {err}");
    }

    /// Concealment params parse from namespaced fields.
    #[test]
    fn parses_concealment() {
        let mut persp = panel_block("p", "root-layout");
        persp.set_property(P_CONCEAL_COMPLETED, true);
        persp.set_property(P_CONCEAL_TAGS, "archived, spam".to_string());
        let blocks = vec![persp];
        let spec = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap();
        assert!(spec.concealment.hide_completed);
        assert_eq!(spec.concealment.hide_tags, vec!["archived", "spam"]);
    }

    /// The default: with no pointer set, the root-layout block is itself the
    /// active perspective.
    #[test]
    fn resolve_defaults_to_root_layout() {
        let mut root = panel_block("root-layout", "root-layout");
        root.parent_id = EntityUri::no_parent();
        root.content = "Default".to_string();
        let blocks = vec![root];
        assert_eq!(
            active_perspective_id(&EntityUri::block("root-layout"), &blocks).unwrap(),
            EntityUri::block("root-layout")
        );
        let spec = resolve_active_perspective(&EntityUri::block("root-layout"), &blocks).unwrap();
        assert_eq!(spec.id, EntityUri::block("root-layout"));
    }

    /// Setting the pointer switches which perspective resolves — the queries
    /// the active perspective exposes change accordingly.
    #[test]
    fn set_and_resolve_switches_active_perspective() {
        let mut root = panel_block("root-layout", "root-layout");
        root.parent_id = EntityUri::no_parent();

        // Default perspective (root-layout's own panel).
        let root_panel = panel_block("root-main", "root-layout");
        let root_src = source_child(
            "root-main::src",
            "root-main",
            QueryLanguage::HolonSql,
            "SELECT 'default'",
        );

        // A second, named perspective with a different query.
        let mut tasks = panel_block("tasks", "root-layout");
        tasks.parent_id = EntityUri::no_parent();
        tasks.set_property(P_NAME, "Tasks".to_string());
        let tasks_panel = panel_block("tasks-main", "tasks");
        let tasks_src = source_child(
            "tasks-main::src",
            "tasks-main",
            QueryLanguage::HolonSql,
            "SELECT 'tasks'",
        );

        set_active_perspective(&mut root, &EntityUri::block("tasks"));

        let blocks = vec![root, root_panel, root_src, tasks, tasks_panel, tasks_src];

        let spec = resolve_active_perspective(&EntityUri::block("root-layout"), &blocks).unwrap();
        assert_eq!(spec.id, EntityUri::block("tasks"));
        assert_eq!(spec.name, "Tasks");
        assert_eq!(spec.panels.len(), 1);
        assert_eq!(
            spec.panels[0].source.as_ref().unwrap().query,
            "SELECT 'tasks'"
        );
    }

    /// The synthesized default layout is byte-identical (modulo whitespace
    /// normalization by the parser) to the previously-hardcoded YAML
    /// `root_layout` variant when applied to the bundled root-layout shape:
    /// left drawer / plain main / right drawer, plus a machinery-only advice
    /// child that must NOT become a column.
    #[test]
    fn layout_dsl_reproduces_bundled_default_shape() {
        let mut root = panel_block("root-layout", "root-layout");
        root.parent_id = EntityUri::no_parent();

        let mut left = panel_block("default-left-sidebar", "root-layout");
        left.set_property("collapse_to", "drawer".to_string());
        let main = panel_block("default-main-panel", "root-layout");
        let mut right = panel_block("default-right-sidebar", "root-layout");
        right.set_property("collapse_to", "drawer".to_string());
        let advice = panel_block("default-advice-rules", "root-layout");

        let blocks = vec![
            root,
            left,
            source_child(
                "left::src",
                "default-left-sidebar",
                QueryLanguage::HolonSql,
                "SELECT 1",
            ),
            main,
            source_child(
                "main::src",
                "default-main-panel",
                QueryLanguage::HolonGql,
                "MATCH (b:block) RETURN b",
            ),
            right,
            source_child(
                "right::src",
                "default-right-sidebar",
                QueryLanguage::HolonGql,
                "MATCH (b:block) RETURN b",
            ),
            advice, // no query/render child → machinery-only, not displayable
        ];

        let spec = resolve_active_perspective(&EntityUri::block("root-layout"), &blocks).unwrap();
        let dsl = spec.layout_dsl().unwrap();

        let expected = concat!(
            "if_space(600,\n  ",
            "bottom_dock(\n    ",
            "columns(drawer(\"block:default-left-sidebar\", \
             live_block(\"block:default-left-sidebar\"), #{mode: \"overlay\"}), ",
            "live_block(\"block:default-main-panel\"), ",
            "drawer(\"block:default-right-sidebar\", live_block(\"block:default-right-sidebar\"), \
             #{mode: \"overlay\"})),\n    ",
            "columns(#{gap: 8, collection: chain_ops(0), item_template: \
             op_button(col(\"name\"))})),\n  ",
            "if_space(1000,\n    ",
            "columns(drawer(\"block:default-left-sidebar\", \
             live_block(\"block:default-left-sidebar\"), #{mode: \"shrink\"}), ",
            "live_block(\"block:default-main-panel\"), ",
            "drawer(\"block:default-right-sidebar\", live_block(\"block:default-right-sidebar\"), \
             #{mode: \"overlay\"})),\n    ",
            "columns(drawer(\"block:default-left-sidebar\", \
             live_block(\"block:default-left-sidebar\"), #{mode: \"shrink\"}), ",
            "live_block(\"block:default-main-panel\"), ",
            "drawer(\"block:default-right-sidebar\", live_block(\"block:default-right-sidebar\"), \
             #{mode: \"shrink\"}))))",
        );
        assert_eq!(dsl, expected);

        // And it parses into a real RenderExpr.
        spec.layout_expr().unwrap();
    }

    /// A perspective whose panels are all machinery-only fails loud rather
    /// than rendering an empty columns() shell.
    #[test]
    fn layout_expr_no_displayable_panels_fails_loud() {
        let mut persp = panel_block("p", "root-layout");
        persp.parent_id = EntityUri::no_parent();
        let bare = panel_block("p-child", "p");
        let blocks = vec![persp, bare];
        let spec = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap();
        let err = spec.layout_expr().unwrap_err();
        assert!(
            err.to_string().contains("no displayable panels"),
            "got: {err}"
        );
    }

    /// `perspective_profile` parses into the profile-cache key (EntityName);
    /// a malformed value fails loud.
    #[test]
    fn profile_override_parses_to_profile_name() {
        let mut persp = panel_block("p", "root-layout");
        persp.set_property(P_PROFILE, "kanban_collection".to_string());
        let panel = panel_block("p-main", "p");
        let src = source_child("p-main::src", "p-main", QueryLanguage::HolonSql, "SELECT 1");
        let blocks = vec![persp, panel, src];
        let spec = PerspectiveSpec::parse(&EntityUri::block("p"), &blocks).unwrap();
        assert_eq!(
            spec.profile_override,
            Some(EntityName::new("kanban_collection"))
        );

        let mut bad = panel_block("q", "root-layout");
        bad.set_property(P_PROFILE, "9 not a name".to_string());
        let err = PerspectiveSpec::parse(&EntityUri::block("q"), &[bad]).unwrap_err();
        assert!(
            err.to_string().contains("not a valid profile name"),
            "got: {err}"
        );
    }

    /// Persistence shape: the pointer is a plain block property, so it
    /// round-trips through the same `properties` map Loro persists (like
    /// collapse state). A property survives a Block clone/serialize the
    /// same way — assert the property is present and drives resolution
    /// after reconstructing the block set from its wire form.
    #[test]
    fn pointer_persists_as_block_property() {
        let mut root = panel_block("root-layout", "root-layout");
        root.parent_id = EntityUri::no_parent();
        set_active_perspective(&mut root, &EntityUri::block("tasks"));
        assert_eq!(
            root.get_property_str(ACTIVE_PERSPECTIVE_PROPERTY)
                .as_deref(),
            Some("block:tasks")
        );
        // Reconstruct from a round-tripped clone (models a reload from storage).
        let reloaded = root.clone();
        assert_eq!(
            active_perspective_id(&EntityUri::block("root-layout"), &[reloaded]).unwrap(),
            EntityUri::block("tasks")
        );
    }
}
