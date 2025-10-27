use crate::arc::{CreateArc, InputArc, OutputArc};
use crate::guard::CompiledExpr;
use crate::{NetDef, TransitionDef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YamlTransition {
    #[serde(skip)]
    pub name: String,
    pub inputs: Vec<InputArc>,
    pub outputs: Vec<OutputArc>,
    #[serde(default)]
    pub creates: Vec<CreateArc>,
    #[serde(default = "default_duration")]
    pub duration: f64,
}

fn default_duration() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjectiveDef {
    pub expr: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default = "default_discount")]
    pub discount_rate: f64,
}

fn default_discount() -> f64 {
    0.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct YamlNetFile {
    pub transitions: BTreeMap<String, YamlTransition>,
    #[serde(default = "default_objective")]
    pub objective: ObjectiveDef,
}

fn default_objective() -> ObjectiveDef {
    ObjectiveDef {
        expr: "0.0".to_string(),
        constraints: vec![],
        discount_rate: 0.0,
    }
}

#[derive(Clone, Debug)]
pub struct YamlNet {
    pub transitions: Vec<YamlTransition>,
    pub objective_def: ObjectiveDef,
    pub compiled_objective: CompiledExpr,
    pub compiled_constraints: Vec<CompiledExpr>,
}

impl YamlNet {
    pub fn new(transitions: Vec<YamlTransition>, objective: ObjectiveDef) -> Result<Self, String> {
        let engine = rhai::Engine::new();
        let compiled_objective = CompiledExpr::compile(&engine, &objective.expr)?;
        let compiled_constraints = objective
            .constraints
            .iter()
            .map(|c| CompiledExpr::compile(&engine, c))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(YamlNet {
            transitions,
            objective_def: objective,
            compiled_objective,
            compiled_constraints,
        })
    }

    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let file: YamlNetFile = serde_yaml::from_str(&content)?;
        let transitions: Vec<YamlTransition> = file
            .transitions
            .into_iter()
            .map(|(name, mut t)| {
                t.name = name;
                t
            })
            .collect();

        let engine = rhai::Engine::new();
        let compiled_objective = CompiledExpr::compile(&engine, &file.objective.expr)?;
        let compiled_constraints = file
            .objective
            .constraints
            .iter()
            .map(|c| CompiledExpr::compile(&engine, c))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(YamlNet {
            transitions,
            objective_def: file.objective,
            compiled_objective,
            compiled_constraints,
        })
    }

    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        for t in &self.transitions {
            let input_binds: Vec<&str> = t.inputs.iter().map(|i| i.bind.as_str()).collect();
            for output in &t.outputs {
                if !input_binds.contains(&output.from.as_str()) {
                    errors.push(format!(
                        "transition '{}': output references unbound name '{}'",
                        t.name, output.from
                    ));
                }
            }
            let consumed_binds: Vec<&str> = t
                .inputs
                .iter()
                .filter(|i| i.consume)
                .map(|i| i.bind.as_str())
                .collect();
            for input in &t.inputs {
                let bind = input.bind.as_str();
                if !consumed_binds.contains(&bind) && !t.outputs.iter().any(|o| o.from == bind) {
                    errors.push(format!(
                        "transition '{}': input binding '{}' not re-produced in any output (and not consumed)",
                        t.name, bind
                    ));
                }
            }
        }
        errors
    }
}

impl TransitionDef for YamlTransition {
    fn id(&self) -> &str {
        &self.name
    }
    fn inputs(&self) -> &[InputArc] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputArc] {
        &self.outputs
    }
    fn creates(&self) -> &[CreateArc] {
        &self.creates
    }
    fn duration_minutes(&self) -> f64 {
        self.duration
    }
}

impl NetDef for YamlNet {
    type Transition = YamlTransition;

    fn transitions(&self) -> Box<dyn Iterator<Item = &YamlTransition> + '_> {
        Box::new(self.transitions.iter())
    }

    fn transition(&self, id: &str) -> Option<&YamlTransition> {
        self.transitions.iter().find(|t| t.name == id)
    }

    fn objective_expr(&self) -> &CompiledExpr {
        &self.compiled_objective
    }

    fn constraints(&self) -> &[CompiledExpr] {
        &self.compiled_constraints
    }

    fn discount_rate(&self) -> f64 {
        self.objective_def.discount_rate
    }
}
