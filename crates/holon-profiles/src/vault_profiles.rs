//! The mirror of the vault's org-embedded profile blocks, fed by whichever
//! block source a session has: the Turso `PROFILE_SQL` view or a snapshot of
//! the Loro tree. Rows are `{id, content}`.

use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::Result;
use futures_signals::signal_map::SignalMapExt;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::live_data::LiveData;

use crate::EntityProfile;
use crate::VaultProfileAdmission;

/// One profile block as a mirror row.
pub fn vault_profile_row(id: &str, content: &str) -> StorageEntity {
    let mut row = StorageEntity::new();
    row.insert("id".into(), Value::String(id.to_string()));
    row.insert("content".into(), Value::String(content.to_string()));
    row
}

/// The two ends of the vault profile mirror.
pub struct VaultProfileMirror {
    /// Every current profile block's content, by block id. The block source
    /// feeds this end.
    pub rows: Arc<LiveData<String>>,
    /// The profiles in effect, by block id. The resolver reads this end.
    pub profiles: Arc<LiveData<EntityProfile>>,
}

/// The mirror over `initial_rows`. Every change of `rows` re-admits the whole
/// set through `admit` ([`crate::TypeRegistry::vault_profile_admitter`]), so
/// `profiles` and the [`holon_api::ConditionKind::ProfileRefused`] conditions
/// are a function of the current rows alone, never of the rows before them.
///
/// Must be called from within a Tokio runtime: the re-admission runs on its
/// own task.
pub fn vault_profile_mirror(
    initial_rows: Vec<StorageEntity>,
    admit: impl Fn(&BTreeMap<String, String>) -> BTreeMap<String, VaultProfileAdmission>
    + Send
    + Sync
    + 'static,
    conditions: Arc<holon_api::ConditionBus>,
) -> VaultProfileMirror {
    let rows = LiveData::new(initial_rows, row_id, row_content);
    let profiles = LiveData::in_memory();
    let mut derivation = VaultProfileDerivation::new(admit, conditions);
    if let Some(in_effect) = derivation.derive(&rows.read()) {
        profiles.replace_all(in_effect);
    }
    let shared = profiles.shared();
    let feed = Arc::downgrade(&rows);
    let signal = rows.signal_map();
    tokio::spawn(signal.for_each(move |_| {
        if let Some(rows) = feed.upgrade() {
            let in_effect = derivation.derive(&rows.read());
            if let Some(in_effect) = in_effect {
                profiles.replace_all(in_effect);
            }
        }
        async {}
    }));
    VaultProfileMirror {
        rows,
        profiles: shared,
    }
}

fn row_id(row: &StorageEntity) -> Result<String> {
    row.get("id")
        .and_then(|v| v.as_string())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("vault profile row has no 'id': {row:?}"))
}

fn row_content(row: &StorageEntity) -> Result<String> {
    row.get("content")
        .and_then(|v| v.as_string())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("vault profile row has no 'content': {row:?}"))
}

/// What the previous admission put in effect and refused, so only changes
/// reach the profiles and the conditions.
pub struct VaultProfileDerivation<A> {
    admit: A,
    conditions: Arc<holon_api::ConditionBus>,
    in_effect: BTreeMap<String, String>,
    /// The refusal conditions raised, error by subject.
    conditions_raised: BTreeMap<String, String>,
}

impl<A> VaultProfileDerivation<A>
where
    A: Fn(&BTreeMap<String, String>) -> BTreeMap<String, VaultProfileAdmission>,
{
    pub fn new(admit: A, conditions: Arc<holon_api::ConditionBus>) -> Self {
        Self {
            admit,
            conditions,
            in_effect: BTreeMap::new(),
            conditions_raised: BTreeMap::new(),
        }
    }

    /// Admit `rows` (content by block id) and update the refusal conditions.
    /// Returns the profiles in effect when they differ from the previous
    /// admission's.
    pub fn derive(
        &mut self,
        rows: &BTreeMap<String, Arc<String>>,
    ) -> Option<BTreeMap<String, Arc<EntityProfile>>> {
        let candidates: BTreeMap<String, String> = rows
            .iter()
            .map(|(id, content)| (id.clone(), content.to_string()))
            .collect();
        let admissions = (self.admit)(&candidates);
        assert!(
            admissions.keys().eq(rows.keys()),
            "the admitter answers for exactly the candidates it was given"
        );

        let conditions: BTreeMap<String, String> = admissions
            .values()
            .filter_map(|a| match a {
                VaultProfileAdmission::Refused { subject, error } => {
                    Some((subject.clone(), error.clone()))
                }
                VaultProfileAdmission::InEffect(_) => None,
            })
            .collect();
        for (subject, error) in &conditions {
            if self.conditions_raised.get(subject) != Some(error) {
                self.conditions.emit(holon_api::Condition {
                    subject: subject.clone(),
                    reason: holon_api::ConditionKind::ProfileRefused {
                        error: error.clone(),
                    },
                });
            }
        }
        for subject in self
            .conditions_raised
            .keys()
            .filter(|subject| !conditions.contains_key(*subject))
        {
            self.conditions.clear(&holon_api::ConditionKey {
                subject: subject.clone(),
                kind: holon_api::ConditionKind::PROFILE_REFUSED,
            });
        }
        self.conditions_raised = conditions;

        let in_effect: BTreeMap<String, String> = candidates
            .into_iter()
            .filter(|(id, _)| matches!(admissions[id], VaultProfileAdmission::InEffect(_)))
            .collect();
        if in_effect == self.in_effect {
            return None;
        }
        self.in_effect = in_effect;
        Some(
            admissions
                .into_iter()
                .filter_map(|(id, a)| match a {
                    VaultProfileAdmission::InEffect(p) => Some((id, Arc::new(p))),
                    VaultProfileAdmission::Refused { .. } => None,
                })
                .collect(),
        )
    }
}
