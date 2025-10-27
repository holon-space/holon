pub mod arc;
pub mod display;
pub mod engine;
pub mod guard;
pub mod objective;
pub mod value;
pub mod yaml;

use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use value::Value;

pub use arc::{CreateArc, InputArc, OutputArc};
pub use guard::CompiledExpr;

pub trait TokenState {
    fn id(&self) -> &str;
    fn token_type(&self) -> &str;
    fn get(&self, attr: &str) -> Option<&Value>;
    fn attrs(&self) -> &BTreeMap<String, Value>;
}

pub trait TransitionDef {
    fn id(&self) -> &str;
    fn inputs(&self) -> &[InputArc];
    fn outputs(&self) -> &[OutputArc];
    fn creates(&self) -> &[CreateArc];
    fn duration_minutes(&self) -> f64;
}

pub trait NetDef {
    type Transition: TransitionDef;
    fn transitions(&self) -> Box<dyn Iterator<Item = &Self::Transition> + '_>;
    fn transition(&self, id: &str) -> Option<&Self::Transition>;
    fn objective_expr(&self) -> &CompiledExpr;
    fn constraints(&self) -> &[CompiledExpr];
    fn discount_rate(&self) -> f64;
}

pub trait Marking: Clone {
    type Token: TokenState;
    fn clock(&self) -> DateTime<Utc>;
    fn set_clock(&mut self, t: DateTime<Utc>);
    fn tokens_of_type(&self, token_type: &str) -> Vec<&Self::Token>;
    fn tokens(&self) -> Box<dyn Iterator<Item = &Self::Token> + '_>;
    fn token(&self, id: &str) -> Option<&Self::Token>;
    fn set_attr(&mut self, token_id: &str, attr: &str, value: Value);
    fn create_token(&mut self, id: String, token_type: String, attrs: BTreeMap<String, Value>);
    fn remove_token(&mut self, id: &str);
}
