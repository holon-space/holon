//! The pure decision core of the advice-rule reconciler (ADR 0022 A1).
//!
//! The engine's reconciler task owns a `DbHandle` and runs DDL sequentially;
//! but the *decision* — given the current slug-ownership map, an incoming rule
//! event, and the parse result, what views to drop / reconcile and what status
//! to record — is a pure function factored out here so it can be unit-tested
//! without an actor or a database.
//!
//! Ownership rules (ADR 0022):
//! - **Slug rename** (a block whose new slug differs from the slug it
//!   previously owned) drops the old `advice_rule_{old_slug}` view first — a
//!   rename must not leave a ghost.
//! - **Unparseable / deleted** blocks tear down whatever view they previously
//!   owned (teardown-on-unparseable matches the oracle's refusal semantics).
//! - **Slug collision** (a second live block claiming a slug a different live
//!   block owns) refuses the newcomer with an error status, does not reconcile
//!   — first-owner-wins.

use std::collections::HashMap;

use crate::rule::AdviceRule;
use crate::rule::AdviceRuleParseError;
use crate::rule::RuleSlug;
use crate::status::AdviceRuleStatus;

/// A rule-block change distilled from a CDC event (the CDC drainer does this
/// cheap conversion; the reconciler task does the YAML parse). `Deleted`
/// carries only the block id because a `Change::Deleted` CDC event carries no
/// content.
#[derive(Debug, Clone, PartialEq)]
pub enum RuleEvent {
    /// A rule block appeared or changed — its current YAML content.
    Upserted { block_id: String, content: String },
    /// A rule block was deleted — only its id survives CDC.
    Deleted { block_id: String },
}

/// What the reconciler task must do for one event, in order: drop the named
/// views first, then (optionally) reconcile a rule, then apply the status.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconcilePlan {
    /// Views to `DROP` before any reconcile (rename / unparseable / delete
    /// teardown).
    pub drops: Vec<RuleSlug>,
    /// The rule to reconcile through `reconcile_advice_rule` (active →
    /// create/diff, inactive → drop). `None` for parse errors, slug
    /// collisions, and deletions.
    pub reconcile: Option<AdviceRule>,
    /// The status to record for the affected block, or `Clear` to forget it.
    pub status: StatusOutcome,
}

/// The status half of a plan. For the reconcile case the status is *intended*
/// Active; the task downgrades it to `DdlError` if the reconcile actually
/// fails.
#[derive(Debug, Clone, PartialEq)]
pub enum StatusOutcome {
    /// Record this status for `block_id`.
    Set {
        block_id: String,
        status: AdviceRuleStatus,
    },
    /// Forget any status for `block_id` (block deleted).
    Clear { block_id: String },
}

/// The reconciler's live ownership state — which block owns which slug, and the
/// inverse. Mutated by [`AdviceReconcilerState::plan`] as events are processed.
#[derive(Debug, Default)]
pub struct AdviceReconcilerState {
    /// block id → the slug it currently owns (parseable, non-colliding rule).
    block_to_slug: HashMap<String, RuleSlug>,
    /// slug string → the block id that owns it (first-owner-wins arbiter).
    slug_to_block: HashMap<String, String>,
}

impl AdviceReconcilerState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Release whatever slug `block_id` currently owns, returning it (for a
    /// drop).
    fn release(&mut self, block_id: &str) -> Option<RuleSlug> {
        if let Some(slug) = self.block_to_slug.remove(block_id) {
            self.slug_to_block.remove(slug.as_str());
            Some(slug)
        } else {
            None
        }
    }

    /// Decide what to do for one rule event, mutating ownership state
    /// accordingly.
    pub fn plan(
        &mut self,
        event: RuleEvent,
        parse: impl Fn(&str) -> Result<AdviceRule, AdviceRuleParseError>,
    ) -> ReconcilePlan {
        match event {
            RuleEvent::Deleted { block_id } => {
                let dropped = self.release(&block_id);
                ReconcilePlan {
                    drops: dropped.into_iter().collect(),
                    reconcile: None,
                    status: StatusOutcome::Clear { block_id },
                }
            }
            RuleEvent::Upserted { block_id, content } => match parse(&content) {
                Err(e) => {
                    // Teardown-on-unparseable: a rule that no longer parses must not
                    // leave its previously-synthesized view live.
                    let dropped = self.release(&block_id);
                    ReconcilePlan {
                        drops: dropped.into_iter().collect(),
                        reconcile: None,
                        status: StatusOutcome::Set {
                            block_id,
                            status: AdviceRuleStatus::ParseError(e.to_string()),
                        },
                    }
                }
                Ok(rule) => {
                    let slug = rule.name.clone();
                    // Slug collision: a *different* live block already owns this slug.
                    if let Some(owner) = self.slug_to_block.get(slug.as_str())
                        && owner != &block_id
                    {
                        let owner = owner.clone();
                        // The newcomer is refused; if it was renaming out of a slug
                        // it owned, that old view is now orphaned — drop it.
                        let dropped = self.release(&block_id);
                        return ReconcilePlan {
                            drops: dropped.into_iter().collect(),
                            reconcile: None,
                            status: StatusOutcome::Set {
                                block_id,
                                status: AdviceRuleStatus::SlugCollision {
                                    slug: slug.as_str().to_string(),
                                    owner,
                                },
                            },
                        };
                    }
                    // Slug free or already ours. If we owned a *different* slug before,
                    // this is a rename — drop the old view first.
                    let mut drops = Vec::new();
                    if let Some(prev) = self.block_to_slug.get(&block_id)
                        && prev.as_str() != slug.as_str()
                        && let Some(old) = self.release(&block_id)
                    {
                        drops.push(old);
                    }
                    self.block_to_slug.insert(block_id.clone(), slug.clone());
                    self.slug_to_block
                        .insert(slug.as_str().to_string(), block_id.clone());
                    ReconcilePlan {
                        drops,
                        reconcile: Some(rule),
                        status: StatusOutcome::Set {
                            block_id,
                            status: AdviceRuleStatus::Active {
                                slug: slug.as_str().to_string(),
                            },
                        },
                    }
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::parse_advice_rule;

    fn rule_yaml(name: &str, active: bool, anchor_tag: &str) -> String {
        format!(
            "name: {name}\nactive: {active}\nk: 3\nanchor:\n  has_tag: \
             {anchor_tag}\ncandidates:\n  tag_overlap_recency:\n    source:\n      has_tag: \
             lesson\n"
        )
    }

    fn upsert(block_id: &str, content: String) -> RuleEvent {
        RuleEvent::Upserted {
            block_id: block_id.to_string(),
            content,
        }
    }

    #[test]
    fn active_rule_reconciles_and_records_active() {
        let mut st = AdviceReconcilerState::new();
        let plan = st.plan(
            upsert("block:1", rule_yaml("r", true, "task")),
            parse_advice_rule,
        );
        assert!(plan.drops.is_empty());
        assert!(plan.reconcile.is_some());
        assert_eq!(
            plan.status,
            StatusOutcome::Set {
                block_id: "block:1".to_string(),
                status: AdviceRuleStatus::Active {
                    slug: "r".to_string()
                },
            }
        );
    }

    #[test]
    fn parse_error_tears_down_previous_view() {
        let mut st = AdviceReconcilerState::new();
        // First, a good rule claims slug `r` and owns a view.
        st.plan(
            upsert("block:1", rule_yaml("r", true, "task")),
            parse_advice_rule,
        );
        // Then the same block becomes unparseable.
        let plan = st.plan(
            upsert("block:1", "not: [valid".to_string()),
            parse_advice_rule,
        );
        assert_eq!(
            plan.drops.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["r"]
        );
        assert!(plan.reconcile.is_none());
        assert!(matches!(
            plan.status,
            StatusOutcome::Set {
                status: AdviceRuleStatus::ParseError(_),
                ..
            }
        ));
        // Ownership released → slug `r` is now free for another block.
        let plan2 = st.plan(
            upsert("block:2", rule_yaml("r", true, "task")),
            parse_advice_rule,
        );
        assert!(plan2.reconcile.is_some());
    }

    #[test]
    fn slug_rename_drops_old_view_first() {
        let mut st = AdviceReconcilerState::new();
        st.plan(
            upsert("block:1", rule_yaml("old", true, "task")),
            parse_advice_rule,
        );
        let plan = st.plan(
            upsert("block:1", rule_yaml("new", true, "task")),
            parse_advice_rule,
        );
        assert_eq!(
            plan.drops.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["old"]
        );
        assert!(plan.reconcile.is_some());
        assert_eq!(
            plan.status,
            StatusOutcome::Set {
                block_id: "block:1".to_string(),
                status: AdviceRuleStatus::Active {
                    slug: "new".to_string()
                },
            }
        );
    }

    #[test]
    fn slug_collision_refuses_newcomer_no_reconcile() {
        let mut st = AdviceReconcilerState::new();
        st.plan(
            upsert("block:1", rule_yaml("shared", true, "task")),
            parse_advice_rule,
        );
        let plan = st.plan(
            upsert("block:2", rule_yaml("shared", true, "note")),
            parse_advice_rule,
        );
        assert!(plan.drops.is_empty());
        assert!(plan.reconcile.is_none(), "collision must not reconcile");
        assert_eq!(
            plan.status,
            StatusOutcome::Set {
                block_id: "block:2".to_string(),
                status: AdviceRuleStatus::SlugCollision {
                    slug: "shared".to_string(),
                    owner: "block:1".to_string(),
                },
            }
        );
    }

    #[test]
    fn re_upsert_same_slug_by_owner_is_not_a_collision() {
        let mut st = AdviceReconcilerState::new();
        st.plan(
            upsert("block:1", rule_yaml("r", true, "task")),
            parse_advice_rule,
        );
        let plan = st.plan(
            upsert("block:1", rule_yaml("r", true, "note")),
            parse_advice_rule,
        );
        assert!(plan.drops.is_empty(), "same-slug re-upsert is not a rename");
        assert!(plan.reconcile.is_some());
    }

    #[test]
    fn delete_drops_owned_view_and_clears_status() {
        let mut st = AdviceReconcilerState::new();
        st.plan(
            upsert("block:1", rule_yaml("r", true, "task")),
            parse_advice_rule,
        );
        let plan = st.plan(
            RuleEvent::Deleted {
                block_id: "block:1".to_string(),
            },
            parse_advice_rule,
        );
        assert_eq!(
            plan.drops.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            vec!["r"]
        );
        assert_eq!(
            plan.status,
            StatusOutcome::Clear {
                block_id: "block:1".to_string()
            }
        );
        // Slug freed → reusable.
        let plan2 = st.plan(
            upsert("block:2", rule_yaml("r", true, "task")),
            parse_advice_rule,
        );
        assert!(plan2.reconcile.is_some());
    }

    #[test]
    fn inactive_rule_still_reconciles_to_tear_down() {
        // An inactive rule routes through reconcile (which DROPs its view) and still
        // owns the slug so a second block can't claim it out from under an edit.
        let mut st = AdviceReconcilerState::new();
        let plan = st.plan(
            upsert("block:1", rule_yaml("r", false, "task")),
            parse_advice_rule,
        );
        assert!(plan.reconcile.is_some());
        let collide = st.plan(
            upsert("block:2", rule_yaml("r", true, "task")),
            parse_advice_rule,
        );
        assert!(matches!(
            collide.status,
            StatusOutcome::Set {
                status: AdviceRuleStatus::SlugCollision { .. },
                ..
            }
        ));
    }
}
