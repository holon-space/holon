use crate::value::Value;
use crate::Marking;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AttrChange {
    pub token: String,
    pub attr: String,
    pub from: Value,
    pub to: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreatedToken {
    pub id: String,
    pub token_type: String,
    pub attrs: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    pub step: usize,
    pub time: DateTime<Utc>,
    pub transition: String,
    pub duration: f64,
    pub changes: Vec<AttrChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub created: Vec<CreatedToken>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct History {
    #[serde(default)]
    pub events: Vec<Event>,
}

impl History {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if !path.exists() {
            return Ok(History { events: vec![] });
        }
        let content = std::fs::read_to_string(path)?;
        if content.trim().is_empty() {
            return Ok(History { events: vec![] });
        }
        let h: History = serde_yaml::from_str(&content)?;
        Ok(h)
    }

    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let content = serde_yaml::to_string(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn append(&mut self, event: Event) {
        self.events.push(event);
    }

    pub fn next_step(&self) -> usize {
        self.events.last().map_or(1, |e| e.step + 1)
    }

    pub fn replay<M: Marking>(&self, initial: &mut M) {
        for event in &self.events {
            for created in &event.created {
                initial.create_token(
                    created.id.clone(),
                    created.token_type.clone(),
                    created.attrs.clone(),
                );
            }
            for change in &event.changes {
                initial.set_attr(&change.token, &change.attr, change.to.clone());
            }
            for removed_id in &event.removed {
                initial.remove_token(removed_id);
            }
            initial.set_clock(event.time + chrono::Duration::minutes(event.duration as i64));
        }
    }
}
