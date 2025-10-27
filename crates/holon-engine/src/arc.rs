use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InputArc {
    pub bind: String,
    pub token_type: String,
    #[serde(default)]
    pub precond: BTreeMap<String, String>,
    #[serde(default)]
    pub consume: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutputArc {
    pub from: String,
    #[serde(default)]
    pub postcond: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CreateArc {
    pub id_expr: String,
    pub token_type: String,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
}
