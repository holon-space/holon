use crate::value::Value;
use crate::{Marking, TokenState};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug)]
pub struct YamlToken {
    pub name: String,
    pub token_type: String,
    pub attributes: BTreeMap<String, Value>,
}

impl TokenState for YamlToken {
    fn id(&self) -> &str {
        &self.name
    }
    fn token_type(&self) -> &str {
        &self.token_type
    }
    fn get(&self, attr: &str) -> Option<&Value> {
        self.attributes.get(attr)
    }
    fn attrs(&self) -> &BTreeMap<String, Value> {
        &self.attributes
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TokenRaw {
    token_type: String,
    #[serde(flatten)]
    attrs: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StateFile {
    clock: DateTime<Utc>,
    tokens: BTreeMap<String, TokenRaw>,
}

#[derive(Clone, Debug)]
pub struct YamlMarking {
    pub clock: DateTime<Utc>,
    pub tokens: Vec<YamlToken>,
}

impl YamlMarking {
    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let file: StateFile = serde_yaml::from_str(&content)?;
        let tokens = file
            .tokens
            .into_iter()
            .map(|(name, raw)| YamlToken {
                name,
                token_type: raw.token_type,
                attributes: raw.attrs,
            })
            .collect();
        Ok(YamlMarking {
            clock: file.clock,
            tokens,
        })
    }

    pub fn save(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let mut tokens = BTreeMap::new();
        for t in &self.tokens {
            tokens.insert(
                t.name.clone(),
                TokenRaw {
                    token_type: t.token_type.clone(),
                    attrs: t.attributes.clone(),
                },
            );
        }
        let file = StateFile {
            clock: self.clock,
            tokens,
        };
        let content = serde_yaml::to_string(&file)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    fn token_mut(&mut self, id: &str) -> &mut YamlToken {
        self.tokens
            .iter_mut()
            .find(|t| t.name == id)
            .unwrap_or_else(|| panic!("token '{id}' not found"))
    }
}

impl Marking for YamlMarking {
    type Token = YamlToken;

    fn clock(&self) -> DateTime<Utc> {
        self.clock
    }

    fn set_clock(&mut self, t: DateTime<Utc>) {
        self.clock = t;
    }

    fn tokens_of_type(&self, token_type: &str) -> Vec<&YamlToken> {
        self.tokens
            .iter()
            .filter(|t| t.token_type == token_type)
            .collect()
    }

    fn tokens(&self) -> Box<dyn Iterator<Item = &YamlToken> + '_> {
        Box::new(self.tokens.iter())
    }

    fn token(&self, id: &str) -> Option<&YamlToken> {
        self.tokens.iter().find(|t| t.name == id)
    }

    fn set_attr(&mut self, token_id: &str, attr: &str, value: Value) {
        self.token_mut(token_id)
            .attributes
            .insert(attr.to_string(), value);
    }

    fn create_token(&mut self, id: String, token_type: String, attrs: BTreeMap<String, Value>) {
        assert!(
            self.tokens.iter().all(|t| t.name != id),
            "token '{id}' already exists"
        );
        self.tokens.push(YamlToken {
            name: id,
            token_type,
            attributes: attrs,
        });
    }

    fn remove_token(&mut self, id: &str) {
        let len_before = self.tokens.len();
        self.tokens.retain(|t| t.name != id);
        assert!(
            self.tokens.len() < len_before,
            "token '{id}' not found for removal"
        );
    }
}
