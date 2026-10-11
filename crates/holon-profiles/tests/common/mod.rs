use std::collections::BTreeMap;

use holon_profiles::TypeRegistry;
use holon_profiles::VaultProfileAdmission;

/// Admit `yaml` as the whole set of vault profiles through the production
/// admitter; the refusal is the admitter's message.
pub fn admit_one(registry: &TypeRegistry, block_id: &str, yaml: &str) -> Result<(), String> {
    let candidates = BTreeMap::from([(block_id.to_string(), yaml.to_string())]);
    let admissions = registry.vault_profile_admitter()(&candidates);
    match admissions
        .get(block_id)
        .expect("the admitter answers every candidate")
    {
        VaultProfileAdmission::InEffect(_) => Ok(()),
        VaultProfileAdmission::Refused(refusal) => Err(refusal.clone()),
    }
}
