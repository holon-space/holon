use crate::plan::Expr;
use crate::row::Datum;
use crate::row::Row;

/// Evaluates an expression of a [`crate::plan::Checked`] plan, whose types
/// the check already proved.
pub(crate) fn eval<R: Row>(expr: &Expr, row: &R, layout: &R::Layout) -> Datum {
    match expr {
        Expr::Col(col) => row.get(layout, *col),
        Expr::Lit(datum) => datum.clone(),
        Expr::Eq(a, b) => Datum::Bool(eval(a, row, layout) == eval(b, row, layout)),
        Expr::Not(e) => Datum::Bool(!holds(e, row, layout)),
    }
}

pub(crate) fn holds<R: Row>(pred: &Expr, row: &R, layout: &R::Layout) -> bool {
    match eval(pred, row, layout) {
        Datum::Bool(b) => b,
        other => unreachable!("a checked predicate yields a Bool, got {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Col;
    use crate::plan::ColType;
    use crate::plan::Schema;
    use crate::row::DynRow;
    use crate::row::Id;

    fn row() -> DynRow {
        DynRow::build(
            &(),
            [
                Datum::Id(Id(7)),
                Datum::Text("a".into()),
                Datum::Bool(false),
            ],
        )
    }

    #[test]
    fn col_reads_its_column() {
        for (i, datum) in [
            Datum::Id(Id(7)),
            Datum::Text("a".into()),
            Datum::Bool(false),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(eval(&Expr::Col(Col(i as u16)), &row(), &()), datum);
        }
    }

    #[test]
    fn lit_is_its_value() {
        let lit = Datum::Text("b".into());
        assert_eq!(eval(&Expr::Lit(lit.clone()), &row(), &()), lit);
    }

    #[test]
    fn eq_compares_values() {
        let layout = DynRow::layout(&Schema(vec![ColType::Id, ColType::Text, ColType::Bool]));
        let same = Expr::Eq(
            Box::new(Expr::Col(Col(1))),
            Box::new(Expr::Lit(Datum::Text("a".into()))),
        );
        let other = Expr::Eq(
            Box::new(Expr::Col(Col(0))),
            Box::new(Expr::Lit(Datum::Id(Id(8)))),
        );
        assert!(holds(&same, &row(), &layout));
        assert!(!holds(&other, &row(), &layout));
    }

    #[test]
    fn not_negates() {
        let not_flag = Expr::Not(Box::new(Expr::Col(Col(2))));
        assert!(holds(&not_flag, &row(), &()));
        assert!(!holds(&Expr::Not(Box::new(not_flag)), &row(), &()));
    }
}
