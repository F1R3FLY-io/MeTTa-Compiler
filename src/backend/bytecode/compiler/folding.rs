//! Constant folding helpers for compile-time evaluation.
//!
//! This module provides functions to evaluate constant expressions
//! at compile time, reducing runtime overhead.

use crate::backend::models::MettaValue;

/// Try to recursively evaluate an expression to a constant at compile time.
/// Returns None if the expression contains variables or other non-constant values.
pub fn try_eval_constant(expr: &MettaValue) -> Option<MettaValue> {
    match expr {
        // Base cases: these are already constants
        MettaValue::Long(_)
        | MettaValue::Float(_)
        | MettaValue::Bool(_)
        | MettaValue::String(_)
        | MettaValue::Nil
        | MettaValue::Unit => Some(expr.clone()),

        // Variables cannot be evaluated at compile time
        MettaValue::Atom(name)
            if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') =>
        {
            None
        }

        // S-expressions need recursive evaluation
        MettaValue::SExpr(items) if !items.is_empty() => {
            if let MettaValue::Atom(op) = &items[0] {
                let args = &items[1..];
                match op.as_str() {
                    // Binary arithmetic
                    "+" | "-" | "*" | "/" | "%" | "mod" | "pow" | "pow-math" | "floor-div"
                        if args.len() == 2 =>
                    {
                        let a = try_eval_constant(&args[0])?;
                        let b = try_eval_constant(&args[1])?;
                        try_fold_binary_arith_values(op, &a, &b)
                    }
                    // Unary arithmetic
                    "abs" | "abs-math" | "neg" if args.len() == 1 => {
                        let a = try_eval_constant(&args[0])?;
                        try_fold_unary_arith(op, &a)
                    }
                    // Comparisons
                    "<" | "<=" | ">" | ">=" | "==" | "!=" if args.len() == 2 => {
                        let a = try_eval_constant(&args[0])?;
                        let b = try_eval_constant(&args[1])?;
                        try_fold_comparison_values(op, &a, &b)
                    }
                    // Boolean operations
                    "and" if args.len() >= 2 => {
                        let consts: Option<Vec<_>> = args.iter().map(try_eval_constant).collect();
                        consts.and_then(|c| try_fold_boolean_values("and", &c))
                    }
                    "or" if args.len() >= 2 => {
                        let consts: Option<Vec<_>> = args.iter().map(try_eval_constant).collect();
                        consts.and_then(|c| try_fold_boolean_values("or", &c))
                    }
                    "not" if args.len() == 1 => {
                        let a = try_eval_constant(&args[0])?;
                        try_fold_boolean_values("not", &[a])
                    }
                    // Conditionals
                    "if" if args.len() == 3 => {
                        let cond = try_eval_constant(&args[0])?;
                        match &cond {
                            MettaValue::Bool(true) => try_eval_constant(&args[1]),
                            MettaValue::Bool(false) => try_eval_constant(&args[2]),
                            _ => None,
                        }
                    }
                    _ => None,
                }
            } else {
                None
            }
        }

        // Everything else can't be evaluated at compile time
        _ => None,
    }
}

/// Try to fold a binary arithmetic operation at compile time
pub fn try_fold_binary_arith(op: &str, a: &MettaValue, b: &MettaValue) -> Option<MettaValue> {
    // First try to evaluate both operands to constants
    let a_const = try_eval_constant(a)?;
    let b_const = try_eval_constant(b)?;
    try_fold_binary_arith_values(op, &a_const, &b_const)
}

/// Inner implementation of binary arithmetic folding on already-constant values
pub fn try_fold_binary_arith_values(
    op: &str,
    a: &MettaValue,
    b: &MettaValue,
) -> Option<MettaValue> {
    match (a, b) {
        (MettaValue::Long(x), MettaValue::Long(y)) => match op {
            "+" => Some(MettaValue::Long(x.wrapping_add(*y))),
            "-" => Some(MettaValue::Long(x.wrapping_sub(*y))),
            "*" => Some(MettaValue::Long(x.wrapping_mul(*y))),
            "/" if *y != 0 => x.checked_div(*y).map(MettaValue::Long),
            "%" | "mod" if *y != 0 => x.checked_rem(*y).map(MettaValue::Long),
            "pow" | "pow-math" if *y >= 0 => x.checked_pow(*y as u32).map(MettaValue::Long),
            "floor-div" if *y != 0 => Some(MettaValue::Long(x.div_euclid(*y))),
            _ => None,
        },
        (MettaValue::Float(x), MettaValue::Float(y)) => match op {
            "+" => Some(MettaValue::Float(x + y)),
            "-" => Some(MettaValue::Float(x - y)),
            "*" => Some(MettaValue::Float(x * y)),
            "/" if *y != 0.0 => Some(MettaValue::Float(x / y)),
            "%" | "mod" if *y != 0.0 => Some(MettaValue::Float(x % y)),
            "pow" | "pow-math" => Some(MettaValue::Float(x.powf(*y))),
            _ => None,
        },
        (MettaValue::Long(x), MettaValue::Float(y)) => {
            let x = *x as f64;
            match op {
                "+" => Some(MettaValue::Float(x + y)),
                "-" => Some(MettaValue::Float(x - y)),
                "*" => Some(MettaValue::Float(x * y)),
                "/" if *y != 0.0 => Some(MettaValue::Float(x / y)),
                "%" | "mod" if *y != 0.0 => Some(MettaValue::Float(x % y)),
                "pow" | "pow-math" => Some(MettaValue::Float(x.powf(*y))),
                _ => None,
            }
        }
        (MettaValue::Float(x), MettaValue::Long(y)) => {
            let y = *y as f64;
            match op {
                "+" => Some(MettaValue::Float(x + y)),
                "-" => Some(MettaValue::Float(x - y)),
                "*" => Some(MettaValue::Float(x * y)),
                "/" if y != 0.0 => Some(MettaValue::Float(x / y)),
                "%" | "mod" if y != 0.0 => Some(MettaValue::Float(x % y)),
                "pow" | "pow-math" => Some(MettaValue::Float(x.powf(y))),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Try to fold a unary arithmetic operation at compile time
pub fn try_fold_unary_arith(op: &str, a: &MettaValue) -> Option<MettaValue> {
    match a {
        MettaValue::Long(x) => {
            match op {
                "abs" | "abs-math" => {
                    // i64::MIN.abs() overflows - let runtime handle the error
                    if *x == i64::MIN {
                        None
                    } else {
                        Some(MettaValue::Long(x.abs()))
                    }
                }
                "neg" => Some(MettaValue::Long(-x)),
                _ => None,
            }
        }
        MettaValue::Float(x) => match op {
            "abs" | "abs-math" => Some(MettaValue::Float(x.abs())),
            "neg" => Some(MettaValue::Float(-x)),
            _ => None,
        },
        _ => None,
    }
}

/// Try to fold a comparison operation at compile time
pub fn try_fold_comparison(op: &str, a: &MettaValue, b: &MettaValue) -> Option<MettaValue> {
    // First try to evaluate both operands to constants
    let a_const = try_eval_constant(a)?;
    let b_const = try_eval_constant(b)?;
    try_fold_comparison_values(op, &a_const, &b_const)
}

/// Inner implementation of comparison folding on already-constant values
pub fn try_fold_comparison_values(op: &str, a: &MettaValue, b: &MettaValue) -> Option<MettaValue> {
    // Helper to compare numeric values
    fn compare_nums(x: f64, y: f64, op: &str) -> Option<MettaValue> {
        match op {
            "<" => Some(MettaValue::Bool(x < y)),
            "<=" => Some(MettaValue::Bool(x <= y)),
            ">" => Some(MettaValue::Bool(x > y)),
            ">=" => Some(MettaValue::Bool(x >= y)),
            "==" => Some(MettaValue::Bool((x - y).abs() < f64::EPSILON)),
            "!=" => Some(MettaValue::Bool((x - y).abs() >= f64::EPSILON)),
            _ => None,
        }
    }

    match (a, b) {
        (MettaValue::Long(x), MettaValue::Long(y)) => match op {
            "<" => Some(MettaValue::Bool(x < y)),
            "<=" => Some(MettaValue::Bool(x <= y)),
            ">" => Some(MettaValue::Bool(x > y)),
            ">=" => Some(MettaValue::Bool(x >= y)),
            "==" => Some(MettaValue::Bool(x == y)),
            "!=" => Some(MettaValue::Bool(x != y)),
            _ => None,
        },
        (MettaValue::Float(x), MettaValue::Float(y)) => compare_nums(*x, *y, op),
        (MettaValue::Long(x), MettaValue::Float(y)) => compare_nums(*x as f64, *y, op),
        (MettaValue::Float(x), MettaValue::Long(y)) => compare_nums(*x, *y as f64, op),
        (MettaValue::Bool(x), MettaValue::Bool(y)) => match op {
            "==" => Some(MettaValue::Bool(x == y)),
            "!=" => Some(MettaValue::Bool(x != y)),
            _ => None,
        },
        (MettaValue::String(x), MettaValue::String(y)) => match op {
            "<" => Some(MettaValue::Bool(x < y)),
            "<=" => Some(MettaValue::Bool(x <= y)),
            ">" => Some(MettaValue::Bool(x > y)),
            ">=" => Some(MettaValue::Bool(x >= y)),
            "==" => Some(MettaValue::Bool(x == y)),
            "!=" => Some(MettaValue::Bool(x != y)),
            _ => None,
        },
        // Nil and Unit comparisons
        (MettaValue::Nil, MettaValue::Nil) => match op {
            "==" => Some(MettaValue::Bool(true)),
            "!=" => Some(MettaValue::Bool(false)),
            _ => None,
        },
        (MettaValue::Unit, MettaValue::Unit) => match op {
            "==" => Some(MettaValue::Bool(true)),
            "!=" => Some(MettaValue::Bool(false)),
            _ => None,
        },
        _ => None,
    }
}

/// Try to fold a boolean operation at compile time
pub fn try_fold_boolean(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    // First try to evaluate all operands to constants
    let consts: Option<Vec<_>> = args.iter().map(try_eval_constant).collect();
    consts.and_then(|c| try_fold_boolean_values(op, &c))
}

/// Inner implementation of boolean folding on already-constant values
///
/// Important: We only fold when BOTH operands are booleans, to preserve
/// type error semantics. Short-circuit evaluation cannot be done at compile
/// time when non-booleans are involved, as we need to preserve runtime errors.
pub fn try_fold_boolean_values(op: &str, args: &[MettaValue]) -> Option<MettaValue> {
    match op {
        "and" => {
            if args.len() != 2 {
                return None;
            }
            // Only fold when both args are booleans to preserve type error semantics
            match (&args[0], &args[1]) {
                (MettaValue::Bool(a), MettaValue::Bool(b)) => Some(MettaValue::Bool(*a && *b)),
                _ => None,
            }
        }
        "or" => {
            if args.len() != 2 {
                return None;
            }
            // Only fold when both args are booleans to preserve type error semantics
            match (&args[0], &args[1]) {
                (MettaValue::Bool(a), MettaValue::Bool(b)) => Some(MettaValue::Bool(*a || *b)),
                _ => None,
            }
        }
        "not" => {
            if args.len() != 1 {
                return None;
            }
            match &args[0] {
                MettaValue::Bool(b) => Some(MettaValue::Bool(!b)),
                _ => None,
            }
        }
        "xor" => {
            if args.len() != 2 {
                return None;
            }
            match (&args[0], &args[1]) {
                (MettaValue::Bool(a), MettaValue::Bool(b)) => Some(MettaValue::Bool(a ^ b)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Check if a value is a constant boolean
#[allow(dead_code)]
pub fn is_const_bool(v: &MettaValue) -> Option<bool> {
    match v {
        MettaValue::Bool(b) => Some(*b),
        MettaValue::Atom(name) if name == "True" => Some(true),
        MettaValue::Atom(name) if name == "False" => Some(false),
        _ => None,
    }
}
