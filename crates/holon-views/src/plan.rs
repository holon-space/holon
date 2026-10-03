//! The logical plan every view is defined in, and its type check. Columns are
//! positions; a [`Checked`] plan carries the schema of every node, and both
//! backends accept only a [`Checked`] plan.
//!
//! The plan has no arithmetic: a value in an output comes from an input or a
//! literal. So the values a recursion can derive are finite, and an `Iterate`
//! with set semantics always reaches its fixed point.

use std::rc::Rc;

use crate::row::Datum;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Col(pub u16);

impl Col {
    pub fn index(self) -> usize {
        usize::from(self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ColType {
    Bool,
    Int,
    Id,
    Text,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Schema(pub Vec<ColType>);

impl Schema {
    pub fn arity(&self) -> usize {
        self.0.len()
    }

    fn col(&self, col: Col) -> Result<ColType, PlanError> {
        self.0
            .get(col.index())
            .copied()
            .ok_or_else(|| PlanError::NoSuchColumn {
                col,
                schema: self.clone(),
            })
    }

    fn pick(&self, cols: &[Col]) -> Result<Schema, PlanError> {
        cols.iter()
            .map(|c| self.col(*c))
            .collect::<Result<_, _>>()
            .map(Schema)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelationId(pub u16);

/// The base relations a plan can scan, by [`RelationId`].
#[derive(Debug, Clone)]
pub struct Catalog {
    pub relations: Vec<Schema>,
}

impl Catalog {
    pub fn schema(&self, relation: RelationId) -> Result<&Schema, PlanError> {
        self.relations
            .get(usize::from(relation.0))
            .ok_or(PlanError::NoSuchRelation(relation))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Col(Col),
    Lit(Datum),
    Eq(Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agg {
    Count,
}

/// One operator over child plans `P`: [`Plan`] as front ends build it,
/// [`Checked`] after the type check.
#[derive(Debug, Clone)]
pub enum Op<P> {
    Scan(RelationId),
    Filter {
        input: P,
        pred: Expr,
    },
    Project {
        input: P,
        exprs: Vec<Expr>,
    },
    /// Inner equi-join; the output row is the left row followed by the right.
    Join {
        left: P,
        right: P,
        keys: Vec<(Col, Col)>,
    },
    /// The output row is the key columns followed by one column per aggregate.
    Reduce {
        input: P,
        key: Vec<Col>,
        aggs: Vec<Agg>,
    },
    /// The least fixed point of `X = distinct(seed ∪ step(X))`.
    Iterate {
        seed: P,
        step: P,
    },
    /// `X` inside the `step` of the enclosing `Iterate`.
    Recur,
}

#[derive(Debug)]
pub struct Plan(pub Op<Rc<Plan>>);

impl Plan {
    pub fn scan(relation: RelationId) -> Rc<Plan> {
        Rc::new(Plan(Op::Scan(relation)))
    }

    pub fn recur() -> Rc<Plan> {
        Rc::new(Plan(Op::Recur))
    }

    pub fn filter(self: &Rc<Self>, pred: Expr) -> Rc<Plan> {
        Rc::new(Plan(Op::Filter {
            input: self.clone(),
            pred,
        }))
    }

    pub fn project(self: &Rc<Self>, exprs: Vec<Expr>) -> Rc<Plan> {
        Rc::new(Plan(Op::Project {
            input: self.clone(),
            exprs,
        }))
    }

    pub fn join(self: &Rc<Self>, right: &Rc<Plan>, keys: Vec<(Col, Col)>) -> Rc<Plan> {
        Rc::new(Plan(Op::Join {
            left: self.clone(),
            right: right.clone(),
            keys,
        }))
    }

    pub fn reduce(self: &Rc<Self>, key: Vec<Col>, aggs: Vec<Agg>) -> Rc<Plan> {
        Rc::new(Plan(Op::Reduce {
            input: self.clone(),
            key,
            aggs,
        }))
    }

    pub fn iterate(self: &Rc<Self>, step: &Rc<Plan>) -> Rc<Plan> {
        Rc::new(Plan(Op::Iterate {
            seed: self.clone(),
            step: step.clone(),
        }))
    }
}

/// A plan that passed [`check`], its only constructor.
///
/// ```compile_fail,E0451
/// use holon_views::plan::{Checked, Op, Schema};
/// let forged = Checked { op: Op::Recur, schema: Schema(vec![]) };
/// ```
#[derive(Debug)]
pub struct Checked {
    op: Op<Rc<Checked>>,
    schema: Schema,
}

impl Checked {
    pub fn op(&self) -> &Op<Rc<Checked>> {
        &self.op
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Whether this sub-plan reads the `Recur` of the enclosing `Iterate`.
    pub fn reads_recur(&self) -> bool {
        match &self.op {
            Op::Recur => true,
            Op::Scan(_) | Op::Iterate { .. } => false,
            Op::Filter { input, .. } | Op::Project { input, .. } | Op::Reduce { input, .. } => {
                input.reads_recur()
            }
            Op::Join { left, right, .. } => left.reads_recur() || right.reads_recur(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PlanError {
    #[error("relation {0:?} is not in the catalog")]
    NoSuchRelation(RelationId),
    #[error("column {col:?} is out of range for {schema:?}")]
    NoSuchColumn { col: Col, schema: Schema },
    #[error("expected a {expected:?} expression, found {found:?}: {expr:?}")]
    ExprType {
        expected: ColType,
        found: ColType,
        expr: Expr,
    },
    #[error("cannot compare {left:?} with {right:?}")]
    Incomparable { left: ColType, right: ColType },
    #[error("`Recur` outside the step of an `Iterate`")]
    RecurOutsideIterate,
    #[error("an `Iterate` inside the step of another `Iterate`")]
    NestedIterate,
    #[error("a `Reduce` reads the recursive relation: its result is not monotone in it")]
    ReduceOverRecur,
    #[error("the step of an `Iterate` yields {step:?}, the seed {seed:?}")]
    StepSchema { seed: Schema, step: Schema },
}

pub fn check(plan: &Rc<Plan>, catalog: &Catalog) -> Result<Rc<Checked>, PlanError> {
    check_in(plan, catalog, None)
}

/// `recur` is the schema of the enclosing `Iterate`, when inside its step.
fn check_in(
    plan: &Rc<Plan>,
    catalog: &Catalog,
    recur: Option<&Schema>,
) -> Result<Rc<Checked>, PlanError> {
    let checked = match &plan.0 {
        Op::Scan(relation) => Checked {
            op: Op::Scan(*relation),
            schema: catalog.schema(*relation)?.clone(),
        },
        Op::Recur => Checked {
            op: Op::Recur,
            schema: recur.ok_or(PlanError::RecurOutsideIterate)?.clone(),
        },
        Op::Filter { input, pred } => {
            let input = check_in(input, catalog, recur)?;
            expect_type(pred, &input.schema, ColType::Bool)?;
            Checked {
                schema: input.schema.clone(),
                op: Op::Filter {
                    input,
                    pred: pred.clone(),
                },
            }
        }
        Op::Project { input, exprs } => {
            let input = check_in(input, catalog, recur)?;
            let schema = exprs
                .iter()
                .map(|e| expr_type(e, &input.schema))
                .collect::<Result<_, _>>()
                .map(Schema)?;
            Checked {
                schema,
                op: Op::Project {
                    input,
                    exprs: exprs.clone(),
                },
            }
        }
        Op::Join { left, right, keys } => {
            let left = check_in(left, catalog, recur)?;
            let right = check_in(right, catalog, recur)?;
            for (l, r) in keys {
                let (lt, rt) = (left.schema.col(*l)?, right.schema.col(*r)?);
                if lt != rt {
                    return Err(PlanError::Incomparable {
                        left: lt,
                        right: rt,
                    });
                }
            }
            let mut schema = left.schema.clone();
            schema.0.extend(right.schema.0.iter().copied());
            Checked {
                schema,
                op: Op::Join {
                    left,
                    right,
                    keys: keys.clone(),
                },
            }
        }
        Op::Reduce { input, key, aggs } => {
            let input = check_in(input, catalog, recur)?;
            if input.reads_recur() {
                return Err(PlanError::ReduceOverRecur);
            }
            let mut schema = input.schema.pick(key)?;
            schema.0.extend(aggs.iter().map(|Agg::Count| ColType::Int));
            Checked {
                schema,
                op: Op::Reduce {
                    input,
                    key: key.clone(),
                    aggs: aggs.clone(),
                },
            }
        }
        Op::Iterate { seed, step } => {
            if recur.is_some() {
                return Err(PlanError::NestedIterate);
            }
            let seed = check_in(seed, catalog, None)?;
            let step = check_in(step, catalog, Some(&seed.schema))?;
            if step.schema != seed.schema {
                return Err(PlanError::StepSchema {
                    seed: seed.schema.clone(),
                    step: step.schema.clone(),
                });
            }
            Checked {
                schema: seed.schema.clone(),
                op: Op::Iterate { seed, step },
            }
        }
    };
    Ok(Rc::new(checked))
}

fn expr_type(expr: &Expr, schema: &Schema) -> Result<ColType, PlanError> {
    match expr {
        Expr::Col(col) => schema.col(*col),
        Expr::Lit(datum) => Ok(datum.col_type()),
        Expr::Eq(a, b) => {
            let (left, right) = (expr_type(a, schema)?, expr_type(b, schema)?);
            if left != right {
                return Err(PlanError::Incomparable { left, right });
            }
            Ok(ColType::Bool)
        }
        Expr::Not(e) => {
            expect_type(e, schema, ColType::Bool)?;
            Ok(ColType::Bool)
        }
    }
}

fn expect_type(expr: &Expr, schema: &Schema, expected: ColType) -> Result<(), PlanError> {
    let found = expr_type(expr, schema)?;
    if found != expected {
        return Err(PlanError::ExprType {
            expected,
            found,
            expr: expr.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog {
            relations: vec![
                Schema(vec![ColType::Id, ColType::Bool]),
                Schema(vec![ColType::Id, ColType::Id]),
            ],
        }
    }

    const NODES: RelationId = RelationId(0);
    const EDGES: RelationId = RelationId(1);

    #[test]
    fn a_filter_needs_a_bool_predicate() {
        let plan = Plan::scan(NODES).filter(Expr::Col(Col(0)));
        assert!(matches!(
            check(&plan, &catalog()),
            Err(PlanError::ExprType {
                expected: ColType::Bool,
                found: ColType::Id,
                ..
            })
        ));
    }

    #[test]
    fn join_keys_must_have_one_type() {
        let plan = Plan::scan(NODES).join(&Plan::scan(EDGES), vec![(Col(1), Col(0))]);
        assert_eq!(
            check(&plan, &catalog()).unwrap_err(),
            PlanError::Incomparable {
                left: ColType::Bool,
                right: ColType::Id
            }
        );
    }

    #[test]
    fn recur_needs_an_enclosing_iterate() {
        let plan = Plan::recur().project(vec![Expr::Col(Col(0))]);
        assert_eq!(
            check(&plan, &catalog()).unwrap_err(),
            PlanError::RecurOutsideIterate
        );
    }

    #[test]
    fn the_step_keeps_the_seed_schema() {
        let seed = Plan::scan(EDGES);
        let step = Plan::recur().project(vec![Expr::Col(Col(0))]);
        assert!(matches!(
            check(&seed.iterate(&step), &catalog()),
            Err(PlanError::StepSchema { .. })
        ));
    }

    #[test]
    fn a_reduce_over_the_recursion_is_refused() {
        let seed = Plan::scan(EDGES);
        let step = Plan::recur()
            .reduce(vec![Col(0)], vec![Agg::Count])
            .join(&Plan::scan(EDGES), vec![(Col(0), Col(0))])
            .project(vec![Expr::Col(Col(0)), Expr::Col(Col(3))]);
        assert_eq!(
            check(&seed.iterate(&step), &catalog()).unwrap_err(),
            PlanError::ReduceOverRecur
        );
    }

    #[test]
    fn an_iterate_inside_a_step_is_refused() {
        let inner = Plan::scan(EDGES).iterate(&Plan::recur());
        let step = Plan::recur()
            .join(&inner, vec![(Col(1), Col(0))])
            .project(vec![Expr::Col(Col(0)), Expr::Col(Col(3))]);
        assert_eq!(
            check(&Plan::scan(EDGES).iterate(&step), &catalog()).unwrap_err(),
            PlanError::NestedIterate
        );
    }
}
