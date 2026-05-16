//! Constant folding helpers for compile-time evaluation.
//!
//! This module provides functions to evaluate constant expressions
//! at compile time, reducing runtime overhead.

use crate::backend::models::{MettaValue, ValueView};

/// Try to recursively evaluate an expression to a constant at compile time.
/// Returns None if the expression contains variables or other non-constant values.
pub fn try_eval_constant(expr: &MettaValue) -> Option<MettaValue> {
    match expr.view() {
        // Base cases: these are already constants
        ValueView::Long(_) | ValueView::Float(_) | ValueView::Bool(_) | ValueView::Unit => {
            Some(expr.clone())
        }

        ValueView::String(_) => Some(expr.clone()),

        // Variables cannot be evaluated at compile time
        ValueView::Atom(name)
            if name.starts_with('$') || name.starts_with('&') || name.starts_with('\'') =>
        {
            None
        }

        // S-expressions need recursive evaluation
        ValueView::SExpr(items) if !items.is_empty() => {
            if let ValueView::Atom(op) = items[0].view() {
                let args = &items[1..];
                match op {
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
                        match cond.view() {
                            ValueView::Bool(true) => try_eval_constant(&args[1]),
                            ValueView::Bool(false) => try_eval_constant(&args[2]),
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
    match (a.view(), b.view()) {
        // Spec §13.2 + §C.7g: silent two's-complement wrap on overflow.
        // The folder mirrors the VM and trampoline runtimes so a literal
        // expression folded at compile time produces the same value as runtime.
        // Division and modulo by zero return None (no fold; runtime raises
        // DivisionByZero).
        (ValueView::Long(x), ValueView::Long(y)) => match op {
            "+" => Some(MettaValue::Long(x.wrapping_add(y))),
            "-" => Some(MettaValue::Long(x.wrapping_sub(y))),
            "*" => Some(MettaValue::Long(x.wrapping_mul(y))),
            "/" if y != 0 => Some(MettaValue::Long(x.wrapping_div(y))),
            "%" | "mod" if y != 0 => Some(MettaValue::Long(x.wrapping_rem(y))),
            // `pow` keeps Long×Long → Long semantics (legacy short-name).
            "pow" if y >= 0 => Some(MettaValue::Long(x.wrapping_pow(y as u32))),
            // `pow-math` is HE-aligned: ALWAYS promote both operands to f64 and
            // return Float, regardless of input subtype (HE stdlib/math.rs:35
            // wraps the result as `Number::Float(res)`).
            "pow-math" => Some(MettaValue::Float((x as f64).powf(y as f64))),
            "floor-div" if y != 0 => Some(MettaValue::Long(x.wrapping_div_euclid(y))),
            _ => None,
        },
        (ValueView::Float(x), ValueView::Float(y)) => match op {
            "+" => Some(MettaValue::Float(x + y)),
            "-" => Some(MettaValue::Float(x - y)),
            "*" => Some(MettaValue::Float(x * y)),
            "/" if y != 0.0 => Some(MettaValue::Float(x / y)),
            "%" | "mod" if y != 0.0 => Some(MettaValue::Float(x % y)),
            "pow" | "pow-math" => Some(MettaValue::Float(x.powf(y))),
            _ => None,
        },
        (ValueView::Long(x), ValueView::Float(y)) => {
            let x = x as f64;
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
        (ValueView::Float(x), ValueView::Long(y)) => {
            let y = y as f64;
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

/// Try to fold a unary arithmetic operation at compile time.
///
/// Spec §13.2: silent two's-complement wrap. abs(i64::MIN) wraps to i64::MIN;
/// neg(i64::MIN) wraps to i64::MIN.
pub fn try_fold_unary_arith(op: &str, a: &MettaValue) -> Option<MettaValue> {
    match a.view() {
        ValueView::Long(x) => match op {
            "abs" | "abs-math" => Some(MettaValue::Long(x.wrapping_abs())),
            "neg" => Some(MettaValue::Long(x.wrapping_neg())),
            _ => None,
        },
        ValueView::Float(x) => match op {
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
            // H4 (2026-05-05) hard-cut: exact f64 == (matches numeric_equal_generic).
            // NaN != NaN per IEEE 754, +0.0 == -0.0 per IEEE 754 — both inherit
            // from Rust's f64 == operator. Tier-divergence avoided.
            "==" => Some(MettaValue::Bool(x == y)),
            "!=" => Some(MettaValue::Bool(x != y)),
            _ => None,
        }
    }

    match (a.view(), b.view()) {
        (ValueView::Long(x), ValueView::Long(y)) => match op {
            "<" => Some(MettaValue::Bool(x < y)),
            "<=" => Some(MettaValue::Bool(x <= y)),
            ">" => Some(MettaValue::Bool(x > y)),
            ">=" => Some(MettaValue::Bool(x >= y)),
            "==" => Some(MettaValue::Bool(x == y)),
            "!=" => Some(MettaValue::Bool(x != y)),
            _ => None,
        },
        (ValueView::Float(x), ValueView::Float(y)) => compare_nums(x, y, op),
        (ValueView::Long(x), ValueView::Float(y)) => compare_nums(x as f64, y, op),
        (ValueView::Float(x), ValueView::Long(y)) => compare_nums(x, y as f64, op),
        (ValueView::Bool(x), ValueView::Bool(y)) => match op {
            "==" => Some(MettaValue::Bool(x == y)),
            "!=" => Some(MettaValue::Bool(x != y)),
            _ => None,
        },
        (ValueView::String(x), ValueView::String(y)) => match op {
            "<" => Some(MettaValue::Bool(x < y)),
            "<=" => Some(MettaValue::Bool(x <= y)),
            ">" => Some(MettaValue::Bool(x > y)),
            ">=" => Some(MettaValue::Bool(x >= y)),
            "==" => Some(MettaValue::Bool(x == y)),
            "!=" => Some(MettaValue::Bool(x != y)),
            _ => None,
        },
        // Unit comparisons
        (ValueView::Unit, ValueView::Unit) => match op {
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
            match (args[0].view(), args[1].view()) {
                (ValueView::Bool(a), ValueView::Bool(b)) => Some(MettaValue::Bool(a && b)),
                _ => None,
            }
        }
        "or" => {
            if args.len() != 2 {
                return None;
            }
            // Only fold when both args are booleans to preserve type error semantics
            match (args[0].view(), args[1].view()) {
                (ValueView::Bool(a), ValueView::Bool(b)) => Some(MettaValue::Bool(a || b)),
                _ => None,
            }
        }
        "not" => {
            if args.len() != 1 {
                return None;
            }
            match args[0].view() {
                ValueView::Bool(b) => Some(MettaValue::Bool(!b)),
                _ => None,
            }
        }
        "xor" => {
            if args.len() != 2 {
                return None;
            }
            match (args[0].view(), args[1].view()) {
                (ValueView::Bool(a), ValueView::Bool(b)) => Some(MettaValue::Bool(a ^ b)),
                _ => None,
            }
        }
        _ => None,
    }
}
