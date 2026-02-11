use std::fmt;

use mettatron::{compile, run_state, MettaState, MettaValue};
use proptest::prelude::*;

#[derive(Clone, Debug)]
struct TestProgram {
    source: String,
    evaluated: MettaValue,
}

#[derive(Clone, Copy, Debug)]
enum ArithOp {
    Add,
    Subtract,
    Multiply,
}

impl ArithOp {
    fn eval(self, lhs: i64, rhs: i64) -> Option<i64> {
        match self {
            ArithOp::Add => lhs.checked_add(rhs),
            ArithOp::Subtract => lhs.checked_sub(rhs),
            ArithOp::Multiply => lhs.checked_mul(rhs),
        }
    }
}

impl fmt::Display for ArithOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let symbol = match self {
            ArithOp::Add => "+",
            ArithOp::Subtract => "-",
            ArithOp::Multiply => "*",
        };
        write!(f, "{}", symbol)
    }
}

impl Arbitrary for ArithOp {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        prop_oneof![
            Just(ArithOp::Add),
            Just(ArithOp::Subtract),
            Just(ArithOp::Multiply),
        ]
        .boxed()
    }
}

#[derive(Clone, Copy, Debug)]
enum ComparOp {
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
    Equal,
    NotEqual,
}

impl ComparOp {
    fn eval(self, lhs: i64, rhs: i64) -> bool {
        match self {
            ComparOp::LessThan => lhs < rhs,
            ComparOp::LessOrEqual => lhs <= rhs,
            ComparOp::GreaterThan => lhs > rhs,
            ComparOp::GreaterOrEqual => lhs >= rhs,
            ComparOp::Equal => lhs == rhs,
            ComparOp::NotEqual => lhs != rhs,
        }
    }

    fn to_condition_source(self, lhs: &str, rhs: &str) -> String {
        match self {
            ComparOp::LessThan => format!("(< {} {})", lhs, rhs),
            ComparOp::LessOrEqual => format!("(<= {} {})", lhs, rhs),
            ComparOp::GreaterThan => format!("(> {} {})", lhs, rhs),
            ComparOp::GreaterOrEqual => format!("(or (> {} {}) (== {} {}))", lhs, rhs, lhs, rhs),
            ComparOp::Equal => format!("(== {} {})", lhs, rhs),
            ComparOp::NotEqual => format!("(not (== {} {}))", lhs, rhs),
        }
    }
}

impl fmt::Display for ComparOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let symbol = match self {
            ComparOp::LessThan => "<",
            ComparOp::LessOrEqual => "<=",
            ComparOp::GreaterThan => ">",
            ComparOp::GreaterOrEqual => ">=",
            ComparOp::Equal => "==",
            ComparOp::NotEqual => "!=",
        };
        write!(f, "{}", symbol)
    }
}

impl Arbitrary for ComparOp {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        prop_oneof![
            Just(ComparOp::LessThan),
            Just(ComparOp::LessOrEqual),
            Just(ComparOp::GreaterThan),
            Just(ComparOp::GreaterOrEqual),
            Just(ComparOp::Equal),
            Just(ComparOp::NotEqual),
        ]
        .boxed()
    }
}

#[derive(Clone, Debug)]
enum ArithExpr {
    Lit(i64),
    BinOp(ArithOp, Box<ArithExpr>, Box<ArithExpr>),
    If(BoolExpr),
}

#[derive(Clone, Debug)]
struct BoolExpr {
    op: ComparOp,
    lhs: Box<ArithExpr>,
    rhs: Box<ArithExpr>,
    then_branch: Box<ArithExpr>,
    else_branch: Box<ArithExpr>,
}

impl BoolExpr {
    fn eval_branch(&self) -> Option<i64> {
        let lhs = self.lhs.eval_checked()?;
        let rhs = self.rhs.eval_checked()?;

        if self.op.eval(lhs, rhs) {
            self.then_branch.eval_checked()
        } else {
            self.else_branch.eval_checked()
        }
    }
}

impl ArithExpr {
    fn eval_checked(&self) -> Option<i64> {
        match self {
            ArithExpr::Lit(value) => Some(*value),
            ArithExpr::BinOp(op, lhs_expr, rhs_expr) => {
                let lhs = lhs_expr.eval_checked()?;
                let rhs = rhs_expr.eval_checked()?;
                op.eval(lhs, rhs)
            }
            ArithExpr::If(bool_expr) => bool_expr.eval_branch(),
        }
    }
}

impl fmt::Display for ArithExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArithExpr::Lit(value) => {
                if *value < 0 {
                    write!(f, "(- 0 {})", value.unsigned_abs())
                } else {
                    write!(f, "{}", value)
                }
            }
            ArithExpr::BinOp(op, lhs_expr, rhs_expr) => {
                write!(f, "({} {} {})", op, lhs_expr, rhs_expr)
            }
            ArithExpr::If(bool_expr) => {
                let lhs = bool_expr.lhs.to_string();
                let rhs = bool_expr.rhs.to_string();
                let condition = bool_expr.op.to_condition_source(&lhs, &rhs);

                write!(
                    f,
                    "(if {} {} {})",
                    condition, bool_expr.then_branch, bool_expr.else_branch
                )
            }
        }
    }
}

impl Arbitrary for ArithExpr {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        fn arithmetic_term() -> BoxedStrategy<ArithExpr> {
            let leaf = (-1000i64..=1000).prop_map(ArithExpr::Lit);

            leaf.prop_recursive(8, 256, 10, |inner| {
                (any::<ArithOp>(), inner.clone(), inner.clone())
                    .prop_map(|(op, lhs, rhs)| ArithExpr::BinOp(op, Box::new(lhs), Box::new(rhs)))
            })
            .boxed()
        }

        let term = arithmetic_term();

        term.clone()
            .prop_recursive(8, 256, 10, move |inner| {
                prop_oneof![(
                    any::<ComparOp>(),
                    term.clone(),
                    term.clone(),
                    inner.clone(),
                    inner.clone()
                )
                    .prop_map(|(op, lhs, rhs, then_branch, else_branch)| {
                        ArithExpr::If(BoolExpr {
                            op,
                            lhs: Box::new(lhs),
                            rhs: Box::new(rhs),
                            then_branch: Box::new(then_branch),
                            else_branch: Box::new(else_branch),
                        })
                    }),]
            })
            .boxed()
    }
}

fn simple_arithmetic() -> impl Strategy<Value = TestProgram> {
    any::<ArithExpr>().prop_filter_map("expression overflows i64 oracle arithmetic", |expr| {
        let evaluated = expr.eval_checked()?;
        let evaluated = MettaValue::SExpr(vec![MettaValue::Long(evaluated)]);

        let source = format!("! ({})", expr);

        Some(TestProgram { source, evaluated })
    })
}

proptest! {
    #[test]
    fn test_simple_arithmetic(test_program in simple_arithmetic()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;
        prop_assert_eq!(result[0].clone(), test_program.evaluated);
    }
}
