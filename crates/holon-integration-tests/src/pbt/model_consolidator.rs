//! The reference model's [`Consolidator`]: a linear history of entity changes.
//!
//! One writer and no merges, so a version is the history's length and
//! ancestry is `<=`. A `Deleted` answer carries the version that performed the
//! delete.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_api::capability::ConsolidatorId;
use holon_core::consolidator::Consolidator;
use holon_core::consolidator::Delta;
use holon_core::consolidator::EntityChange;
use holon_core::consolidator::ReplicaId;
use holon_core::consolidator::Seen;
use holon_core::consolidator::Version;
use holon_core::traits::Result;

const EPOCH: &str = "keystone-model";

#[derive(Default)]
pub struct ModelConsolidator {
    history: Mutex<Vec<(EntityUri, EntityChange)>>,
    bases: Mutex<BTreeMap<ReplicaId, Version>>,
}

impl ModelConsolidator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one change; returns the version it produced.
    pub fn record(&self, id: EntityUri, change: EntityChange) -> Version {
        let mut history = self.history.lock().unwrap();
        history.push((id, change));
        version_at(history.len())
    }
}

fn version_at(len: usize) -> Version {
    Version::new(len.to_string())
}

fn position(version: &Version) -> Result<usize> {
    version
        .as_str()
        .parse()
        .map_err(|e| format!("{version} is not a keystone-model version: {e}").into())
}

#[async_trait]
impl Consolidator for ModelConsolidator {
    async fn is_ancestor(&self, a: &Version, b: &Version) -> Result<bool> {
        let (a, b) = (position(a)?, position(b)?);
        let len = self.history.lock().unwrap().len();
        if a > len || b > len {
            return Err(format!("version beyond head {len}: {a}, {b}").into());
        }
        Ok(a <= b)
    }

    fn epoch_id(&self) -> ConsolidatorId {
        ConsolidatorId::new(EPOCH)
    }

    async fn head(&self) -> Result<Version> {
        Ok(version_at(self.history.lock().unwrap().len()))
    }

    async fn diff(&self, from: &Version, to: &Version) -> Result<Delta> {
        let (from, to) = (position(from)?, position(to)?);
        let history = self.history.lock().unwrap();
        if from > to || to > history.len() {
            return Err(format!(
                "diff({from}, {to}) is not a forward range within head {}",
                history.len()
            )
            .into());
        }
        let mut order: Vec<EntityUri> = Vec::new();
        let mut net: HashMap<EntityUri, EntityChange> = HashMap::new();
        for (id, change) in &history[from..to] {
            if !net.contains_key(id) {
                order.push(id.clone());
            }
            net.insert(id.clone(), change.clone());
        }
        Ok(Delta {
            changes: order
                .into_iter()
                .map(|id| {
                    let change = net.remove(&id).expect("every ordered id has a net change");
                    (id, change)
                })
                .collect(),
        })
    }

    async fn ever_seen(&self, id: &EntityUri) -> Result<Seen> {
        let history = self.history.lock().unwrap();
        let last = history
            .iter()
            .enumerate()
            .rev()
            .find(|(_, (entity, _))| entity == id);
        Ok(match last {
            None => Seen::Never,
            Some((idx, (_, EntityChange::Deleted))) => Seen::Deleted(version_at(idx + 1)),
            Some(_) => Seen::Live,
        })
    }

    async fn register_base(&self, replica: &ReplicaId, at: &Version) -> Result<()> {
        position(at)?;
        self.bases
            .lock()
            .unwrap()
            .insert(replica.clone(), at.clone());
        Ok(())
    }

    async fn release_base(&self, replica: &ReplicaId) -> Result<()> {
        self.bases
            .lock()
            .unwrap()
            .remove(replica)
            .map(|_| ())
            .ok_or_else(|| format!("release_base: {} holds no base", replica.as_str()).into())
    }

    async fn registered_bases(&self) -> Result<Vec<ReplicaId>> {
        Ok(self.bases.lock().unwrap().keys().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ever_seen_separates_a_tombstone_from_a_never() {
        let model = ModelConsolidator::new();
        let id = EntityUri::block("x");
        assert_eq!(model.ever_seen(&id).await.unwrap(), Seen::Never);

        model.record(
            id.clone(),
            EntityChange::Created {
                fields: HashMap::new(),
            },
        );
        assert_eq!(model.ever_seen(&id).await.unwrap(), Seen::Live);

        let deleted_at = model.record(id.clone(), EntityChange::Deleted);
        assert_eq!(
            model.ever_seen(&id).await.unwrap(),
            Seen::Deleted(deleted_at)
        );
        assert!(!model.ever_seen(&id).await.unwrap().admits_adoption());
    }

    #[tokio::test]
    async fn diff_nets_each_entity_to_its_last_change() {
        let model = ModelConsolidator::new();
        let start = model.head().await.unwrap();
        let (a, b) = (EntityUri::block("a"), EntityUri::block("b"));
        model.record(
            a.clone(),
            EntityChange::Created {
                fields: HashMap::new(),
            },
        );
        model.record(
            b.clone(),
            EntityChange::Created {
                fields: HashMap::new(),
            },
        );
        model.record(a.clone(), EntityChange::Deleted);
        let head = model.head().await.unwrap();

        assert!(model.is_ancestor(&start, &head).await.unwrap());
        assert!(!model.is_ancestor(&head, &start).await.unwrap());
        let delta = model.diff(&start, &head).await.unwrap();
        assert_eq!(
            delta.changes,
            vec![
                (a, EntityChange::Deleted),
                (
                    b,
                    EntityChange::Created {
                        fields: HashMap::new()
                    }
                ),
            ]
        );
    }
}
