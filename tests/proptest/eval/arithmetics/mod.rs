use std::fmt;

use mettatron::{compile, run_state, MettaState, MettaValue};
use proptest::prelude::*;

mod operators;

use self::operators::{ArithOp, ComparOp};

#[derive(Clone, Debug)]
struct TestProgram {
    source: String,
    evaluated: MettaValue,
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
            ArithExpr::Lit(value) => write!(f, "{}", format_i64_literal(*value)),
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
        let leaf = (-1000i64..=1000).prop_map(ArithExpr::Lit);

        leaf.prop_recursive(8, 256, 10, |inner| {
            prop_oneof![
                (any::<ArithOp>(), inner.clone(), inner.clone())
                    .prop_map(|(op, lhs, rhs)| ArithExpr::BinOp(op, Box::new(lhs), Box::new(rhs))),
                (
                    any::<ComparOp>(),
                    inner.clone(),
                    inner.clone(),
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
                    }),
            ]
        })
        .boxed()
    }
}

fn format_i64_literal(value: i64) -> String {
    if value < 0 {
        format!("(- 0 {})", value.unsigned_abs())
    } else {
        value.to_string()
    }
}

fn invalid_atom_symbol() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("A".to_string()),
        Just("B".to_string()),
        Just("C".to_string()),
    ]
}

// Will generate programs like: ! ((+ (if (not (== (- 0 919) 260)) 777 (- 0 421)) (- 0 413)))
fn valid_arithmetics_with_conditions() -> impl Strategy<Value = TestProgram> {
    any::<ArithExpr>().prop_filter_map("expression overflows i64 oracle arithmetic", |expr| {
        let evaluated = expr.eval_checked()?;
        let evaluated = MettaValue::SExpr(vec![MettaValue::Long(evaluated)]);

        let source = format!("! ({})", expr);

        Some(TestProgram { source, evaluated })
    })
}

// Inject symbols (A/B/C) into arithmetic operator arguments to provoke TypeError.
fn invalid_arithmetic_programs() -> impl Strategy<Value = String> {
    (
        any::<ArithOp>(),
        invalid_atom_symbol(),
        -1000i64..=1000i64,
        any::<bool>(),
    )
        .prop_map(|(op, symbol, number, symbol_on_lhs)| {
            let number = format_i64_literal(number);
            let (lhs, rhs) = if symbol_on_lhs {
                (symbol, number)
            } else {
                (number, symbol)
            };

            format!("! (({} {} {}))", op, lhs, rhs)
        })
}

// Inject symbols (A/B/C) into numeric comparison in condition to provoke TypeError.
fn invalid_comparison_programs() -> impl Strategy<Value = String> {
    (
        any::<ComparOp>(),
        invalid_atom_symbol(),
        -1000i64..=1000i64,
        any::<bool>(),
    )
        .prop_map(|(op, symbol, number, symbol_on_lhs)| {
            let number = format_i64_literal(number);
            let (lhs, rhs) = if symbol_on_lhs {
                (symbol, number)
            } else {
                (number, symbol)
            };

            let condition = op.to_condition_source(&lhs, &rhs);
            format!("! ((if {} (+ 3 2) B))", condition)
        })
}

fn invalid_arithmetic_arity_programs() -> impl Strategy<Value = String> {
    let literal = (-1000i64..=1000i64).prop_map(format_i64_literal);

    prop_oneof![
        // Examples: (+ 3 2 2), (* 1 9 4), ...
        (
            any::<ArithOp>(),
            literal.clone(),
            literal.clone(),
            literal.clone(),
        )
            .prop_map(|(op, a, b, c)| format!("! (({} {} {} {}))", op, a, b, c)),
        // Examples: (+ + 3 3), (- * 5 8), ...
        (any::<ArithOp>(), any::<ArithOp>(), literal.clone(), literal)
            .prop_map(|(outer, inner, a, b)| format!("! (({} {} {} {}))", outer, inner, a, b)),
    ]
}

#[derive(Clone, Debug)]
struct WrongOrderingCase {
    source: String,
    expected: MettaValue,
}

fn wrong_operator_ordering_programs() -> impl Strategy<Value = WrongOrderingCase> {
    // Positive literals keep output shape stable as Long(...) atoms.
    (0i64..=1000i64, any::<ArithOp>(), 0i64..=1000i64).prop_map(|(lhs, op, rhs)| {
        let op_symbol = op.to_string();
        WrongOrderingCase {
            source: format!("! (({} {} {}))", lhs, op_symbol, rhs),
            expected: MettaValue::SExpr(vec![MettaValue::SExpr(vec![
                MettaValue::Long(lhs),
                MettaValue::Atom(op_symbol),
                MettaValue::Long(rhs),
            ])]),
        }
    })
}

proptest! {
    #[test]
    fn test_valid_arithmetics_with_conditions(test_program in valid_arithmetics_with_conditions()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;
        prop_assert_eq!(result[0].clone(), test_program.evaluated);
    }

    #[test]
    fn test_arithmetic_type_errors(src in invalid_arithmetic_programs()) {
        let state = MettaState::new_empty();
        let compiled = compile(&src);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;
        prop_assert_eq!(result.len(), 1);

        match &result[0] {
            MettaValue::Error(message, detail) => {
                prop_assert!(message.contains("Cannot perform"));
                prop_assert!(message.contains("expected Number (integer), got Atom"));
                prop_assert_eq!(detail.as_ref(), &MettaValue::Atom("TypeError".to_string()));
            }
            other => prop_assert!(false, "expected Error(TypeError), got {other:?}"),
        }
    }

    #[test]
    fn test_comparison_type_errors(src in invalid_comparison_programs()) {
        let state = MettaState::new_empty();
        let compiled = compile(&src);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;
        prop_assert_eq!(result.len(), 1);

        match &result[0] {
            MettaValue::Error(message, detail) => {
                prop_assert!(message.contains("Cannot compare: expected Number (integer), got Atom"));
                prop_assert_eq!(detail.as_ref(), &MettaValue::Atom("TypeError".to_string()));
            }
            other => prop_assert!(false, "expected Error(TypeError), got {other:?}"),
        }
    }

    #[test]
    fn test_wrong_number_of_arguments_errors(src in invalid_arithmetic_arity_programs()) {
        let state = MettaState::new_empty();
        let compiled = compile(&src);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;
        prop_assert_eq!(result.len(), 1);

        match &result[0] {
            MettaValue::Error(message, _) => {
                prop_assert!(message.contains("requires exactly 2 arguments"));
            }
            other => prop_assert!(false, "expected arity Error, got {other:?}"),
        }
    }

    #[test]
    fn test_wrong_operator_ordering_stays_unreduced(case in wrong_operator_ordering_programs()) {
        let state = MettaState::new_empty();
        let compiled = compile(&case.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;
        prop_assert_eq!(result.len(), 1);
        prop_assert_eq!(result[0].clone(), case.expected);
    }
}
