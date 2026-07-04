//! The read-only plan behind the `merge_blocks` compound.
//!
//! The provider computes it (it needs DB reads); the engine executes it as a
//! sequence of ordinary invertible ops. Preconditions are checked HERE, before
//! any write, so a refused merge leaves no partial state.

use std::collections::HashMap;

use holon_api::Value;

/// The property holding a block's merge provenance — space-separated
/// `<merged-away-id> <millis>` pairs. This is the REPLICATED redirect record;
/// the `block_redirects` table is its queryable index.
pub const MERGED_FROM_FIELD: &str = "merged_from";

/// The dedupe key: trim, then collapse each whitespace run to one space.
pub fn normalize_content(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse `merged_from` into its `(merged-away id, millis)` pairs. Fails loud
/// on a malformed value rather than dropping provenance.
pub fn parse_merged_from(value: &Value) -> Result<Vec<(String, i64)>, String> {
    let raw = match value {
        Value::Null => return Ok(Vec::new()),
        Value::String(s) => s.clone(),
        other => {
            return Err(format!(
                "{MERGED_FROM_FIELD}: expected String or Null, got {other:?}"
            ));
        }
    };
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    if !tokens.len().is_multiple_of(2) {
        return Err(format!(
            "{MERGED_FROM_FIELD}: expected `<id> <millis>` pairs, got an odd token count in {raw:?}"
        ));
    }
    tokens
        .chunks(2)
        .map(|pair| {
            let millis = pair[1].parse::<i64>().map_err(|e| {
                format!(
                    "{MERGED_FROM_FIELD}: {:?} is not a millis timestamp: {e}",
                    pair[1]
                )
            })?;
            Ok((pair[0].to_string(), millis))
        })
        .collect()
}

/// Render `(id, millis)` pairs back into the property's string form.
pub fn render_merged_from(entries: &[(String, i64)]) -> String {
    entries
        .iter()
        .map(|(id, at)| format!("{id} {at}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// One direct child of either side, in sibling order.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanChild {
    pub id: String,
    pub content: String,
    /// Whether the id was authored in an org file (an `:ID:` a human wrote)
    /// rather than minted. Authored ids win a dedupe collapse.
    pub authored: bool,
    pub created_at: i64,
}

/// A collapsed duplicate: its own children are re-homed under the keeper
/// before it is deleted behind its redirect.
#[derive(Debug, Clone, PartialEq)]
pub struct DedupeLoser {
    pub id: String,
    /// The loser's direct children, in sibling order.
    pub children: Vec<String>,
}

/// A dedupe collapse: `keeper` absorbs `losers`.
#[derive(Debug, Clone, PartialEq)]
pub struct DedupeGroup {
    pub keeper: String,
    /// The keeper's last existing child — the anchor re-homed children append
    /// after.
    pub keeper_last_child: Option<String>,
    /// The keeper's own merge provenance, which the collapse appends to.
    pub keeper_merged_from: Vec<(String, i64)>,
    pub losers: Vec<DedupeLoser>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergeBlocksPlan {
    pub canonical_id: String,
    pub duplicate_id: String,
    pub canonical_content: String,
    pub duplicate_content: String,
    /// The canonical's children, then the duplicate's — the deterministic
    /// post-merge order before dedupe collapses run.
    pub merged_children: Vec<PlanChild>,
    /// How many of `merged_children` are the canonical's own; the rest are the
    /// duplicate's, and are the ones that must be moved.
    pub canonical_child_count: i64,
    pub dedupe_groups: Vec<DedupeGroup>,
    /// `merged_from` pairs the canonical already carries, which the new entry
    /// is appended to (append-only).
    pub existing_merged_from: Vec<(String, i64)>,
    /// Tags the canonical must end up with: the union of both sides.
    pub union_tags: Vec<String>,
    /// Properties present on the duplicate but not the canonical — canonical
    /// wins every conflict, so only these are copied over.
    pub adopted_properties: Vec<(String, Value)>,
    pub merged_at: i64,
}

fn get_str(obj: &HashMap<String, Value>, key: &str) -> Result<String, String> {
    match obj.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        other => Err(format!(
            "MergeBlocksPlan: '{key}' must be a String, got {other:?}"
        )),
    }
}

fn get_i64(obj: &HashMap<String, Value>, key: &str) -> Result<i64, String> {
    match obj.get(key) {
        Some(Value::Integer(i)) => Ok(*i),
        other => Err(format!(
            "MergeBlocksPlan: '{key}' must be an Integer, got {other:?}"
        )),
    }
}

fn get_array<'a>(obj: &'a HashMap<String, Value>, key: &str) -> Result<&'a Vec<Value>, String> {
    match obj.get(key) {
        Some(Value::Array(items)) => Ok(items),
        other => Err(format!(
            "MergeBlocksPlan: '{key}' must be an Array, got {other:?}"
        )),
    }
}

fn as_object<'a>(value: &'a Value, what: &str) -> Result<&'a HashMap<String, Value>, String> {
    match value {
        Value::Object(o) => Ok(o),
        other => Err(format!(
            "MergeBlocksPlan: {what} must be an Object, got {other:?}"
        )),
    }
}

impl MergeBlocksPlan {
    pub fn to_value(&self) -> Value {
        let mut obj = HashMap::new();
        obj.insert(
            "canonical_id".into(),
            Value::String(self.canonical_id.clone()),
        );
        obj.insert(
            "duplicate_id".into(),
            Value::String(self.duplicate_id.clone()),
        );
        obj.insert(
            "canonical_content".into(),
            Value::String(self.canonical_content.clone()),
        );
        obj.insert(
            "duplicate_content".into(),
            Value::String(self.duplicate_content.clone()),
        );
        obj.insert(
            "merged_children".into(),
            Value::Array(
                self.merged_children
                    .iter()
                    .map(|c| {
                        let mut o = HashMap::new();
                        o.insert("id".to_string(), Value::String(c.id.clone()));
                        o.insert("content".to_string(), Value::String(c.content.clone()));
                        o.insert(
                            "authored".to_string(),
                            Value::Integer(i64::from(c.authored)),
                        );
                        o.insert("created_at".to_string(), Value::Integer(c.created_at));
                        Value::Object(o)
                    })
                    .collect(),
            ),
        );
        obj.insert(
            "dedupe_groups".into(),
            Value::Array(
                self.dedupe_groups
                    .iter()
                    .map(|g| {
                        let mut o = HashMap::new();
                        o.insert("keeper".to_string(), Value::String(g.keeper.clone()));
                        o.insert(
                            "keeper_last_child".to_string(),
                            match &g.keeper_last_child {
                                Some(id) => Value::String(id.clone()),
                                None => Value::Null,
                            },
                        );
                        o.insert(
                            "keeper_merged_from".to_string(),
                            Value::String(render_merged_from(&g.keeper_merged_from)),
                        );
                        o.insert(
                            "losers".to_string(),
                            Value::Array(
                                g.losers
                                    .iter()
                                    .map(|l| {
                                        let mut lo = HashMap::new();
                                        lo.insert("id".to_string(), Value::String(l.id.clone()));
                                        lo.insert(
                                            "children".to_string(),
                                            Value::Array(
                                                l.children
                                                    .iter()
                                                    .map(|c| Value::String(c.clone()))
                                                    .collect(),
                                            ),
                                        );
                                        Value::Object(lo)
                                    })
                                    .collect(),
                            ),
                        );
                        Value::Object(o)
                    })
                    .collect(),
            ),
        );
        obj.insert(
            "existing_merged_from".into(),
            Value::String(render_merged_from(&self.existing_merged_from)),
        );
        obj.insert(
            "union_tags".into(),
            Value::Array(
                self.union_tags
                    .iter()
                    .map(|t| Value::String(t.clone()))
                    .collect(),
            ),
        );
        obj.insert(
            "adopted_properties".into(),
            Value::Array(
                self.adopted_properties
                    .iter()
                    .map(|(k, v)| {
                        let mut o = HashMap::new();
                        o.insert("key".to_string(), Value::String(k.clone()));
                        o.insert("value".to_string(), v.clone());
                        Value::Object(o)
                    })
                    .collect(),
            ),
        );
        obj.insert("merged_at".into(), Value::Integer(self.merged_at));
        obj.insert(
            "canonical_child_count".into(),
            Value::Integer(self.canonical_child_count),
        );
        Value::Object(obj)
    }

    pub fn from_value(value: &Value) -> Result<Self, String> {
        let obj = as_object(value, "plan")?;
        let merged_children = get_array(obj, "merged_children")?
            .iter()
            .map(|item| {
                let c = as_object(item, "merged child")?;
                Ok(PlanChild {
                    id: get_str(c, "id")?,
                    content: get_str(c, "content")?,
                    authored: get_i64(c, "authored")? != 0,
                    created_at: get_i64(c, "created_at")?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let dedupe_groups = get_array(obj, "dedupe_groups")?
            .iter()
            .map(|item| {
                let g = as_object(item, "dedupe group")?;
                let losers = get_array(g, "losers")?
                    .iter()
                    .map(|l| {
                        let lo = as_object(l, "dedupe loser")?;
                        let children = get_array(lo, "children")?
                            .iter()
                            .map(|c| match c {
                                Value::String(s) => Ok(s.clone()),
                                other => Err(format!(
                                    "MergeBlocksPlan: loser child must be String, got {other:?}"
                                )),
                            })
                            .collect::<Result<Vec<_>, String>>()?;
                        Ok(DedupeLoser {
                            id: get_str(lo, "id")?,
                            children,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                Ok(DedupeGroup {
                    keeper: get_str(g, "keeper")?,
                    keeper_last_child: match g.get("keeper_last_child") {
                        Some(Value::String(s)) => Some(s.clone()),
                        Some(Value::Null) | None => None,
                        other => {
                            return Err(format!(
                                "MergeBlocksPlan: 'keeper_last_child' must be String or Null, got \
                                 {other:?}"
                            ));
                        }
                    },
                    keeper_merged_from: parse_merged_from(&Value::String(get_str(
                        g,
                        "keeper_merged_from",
                    )?))?,
                    losers,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let union_tags = get_array(obj, "union_tags")?
            .iter()
            .map(|t| match t {
                Value::String(s) => Ok(s.clone()),
                other => Err(format!(
                    "MergeBlocksPlan: tag must be String, got {other:?}"
                )),
            })
            .collect::<Result<Vec<_>, String>>()?;
        let adopted_properties = get_array(obj, "adopted_properties")?
            .iter()
            .map(|item| {
                let p = as_object(item, "adopted property")?;
                let value = p.get("value").cloned().ok_or_else(|| {
                    "MergeBlocksPlan: adopted property missing 'value'".to_string()
                })?;
                Ok((get_str(p, "key")?, value))
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            canonical_id: get_str(obj, "canonical_id")?,
            duplicate_id: get_str(obj, "duplicate_id")?,
            canonical_content: get_str(obj, "canonical_content")?,
            duplicate_content: get_str(obj, "duplicate_content")?,
            merged_children,
            dedupe_groups,
            existing_merged_from: parse_merged_from(&Value::String(get_str(
                obj,
                "existing_merged_from",
            )?))?,
            union_tags,
            adopted_properties,
            merged_at: get_i64(obj, "merged_at")?,
            canonical_child_count: get_i64(obj, "canonical_child_count")?,
        })
    }
}

/// Group `children` by normalized content and pick each group's keeper:
/// an authored `:ID:` beats a minted one, then the oldest `created_at`, then
/// the id (so the choice is total and reproducible). Groups of one collapse
/// nothing and are omitted. Returns `(keeper, losers)` id pairs; the caller
/// enriches them with the reads a `DedupeGroup` needs.
pub fn plan_dedupe(children: &[PlanChild]) -> Vec<(String, Vec<String>)> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<&PlanChild>> = HashMap::new();
    for child in children {
        let key = normalize_content(&child.content);
        // Husks are not duplicates of one another — an empty block carries no
        // identity to collapse.
        if key.is_empty() {
            continue;
        }
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(child);
    }
    let mut out = Vec::new();
    for key in order {
        let members = &groups[&key];
        if members.len() < 2 {
            continue;
        }
        let keeper = members
            .iter()
            .min_by(|a, b| {
                b.authored
                    .cmp(&a.authored)
                    .then(a.created_at.cmp(&b.created_at))
                    .then(a.id.cmp(&b.id))
            })
            .expect("a group with >= 2 members has a minimum");
        out.push((
            keeper.id.clone(),
            members
                .iter()
                .filter(|m| m.id != keeper.id)
                .map(|m| m.id.clone())
                .collect(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn child(id: &str, content: &str, authored: bool, created_at: i64) -> PlanChild {
        PlanChild {
            id: id.to_string(),
            content: content.to_string(),
            authored,
            created_at,
        }
    }

    #[test]
    fn normalization_collapses_whitespace_runs() {
        assert_eq!(normalize_content("  alpha \t one "), "alpha one");
        assert_eq!(normalize_content("   "), "");
    }

    #[test]
    fn authored_id_wins_over_older_minted_one() {
        let groups = plan_dedupe(&[
            child("minted", "alpha one", false, 1),
            child("authored", "alpha  one", true, 9),
        ]);
        assert_eq!(
            groups,
            vec![("authored".to_string(), vec!["minted".to_string()])]
        );
    }

    #[test]
    fn oldest_wins_among_equally_authored_ids() {
        let groups = plan_dedupe(&[
            child("young", "alpha one", false, 9),
            child("old", "alpha one", false, 1),
        ]);
        assert_eq!(groups[0].0, "old");
    }

    #[test]
    fn husks_and_singletons_collapse_nothing() {
        let groups = plan_dedupe(&[
            child("a", "", false, 1),
            child("b", "  ", false, 2),
            child("c", "alpha one", false, 3),
        ]);
        assert!(groups.is_empty());
    }

    #[test]
    fn merged_from_round_trips() {
        let entries = vec![("dup-a".to_string(), 17i64), ("dup-b".to_string(), 42i64)];
        let rendered = render_merged_from(&entries);
        assert_eq!(rendered, "dup-a 17 dup-b 42");
        assert_eq!(
            parse_merged_from(&Value::String(rendered)).unwrap(),
            entries
        );
    }

    #[test]
    fn malformed_merged_from_fails_loud() {
        assert!(parse_merged_from(&Value::String("dup-a".into())).is_err());
        assert!(parse_merged_from(&Value::String("dup-a later".into())).is_err());
    }
}
