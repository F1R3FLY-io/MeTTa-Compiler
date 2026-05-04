use std::fmt;

use proptest::prelude::*;

#[derive(Clone, Copy, Debug)]
pub(super) enum ArithOp {
    Add,
    Subtract,
    Multiply,
}

impl ArithOp {
    /// Oracle for the proptest harness: matches MeTTa spec §13.2 + §C.7g
    /// silent two's-complement wrap semantics. Mirrors
    /// `src/backend/grounded/arithmetic.rs` (trampoline tier),
    /// `src/backend/bytecode/vm/mod.rs` (bytecode VM tier), and
    /// `src/backend/bytecode/jit/runtime/arithmetic.rs` (JIT tier).
    pub(super) fn eval(self, lhs: i64, rhs: i64) -> i64 {
        match self {
            ArithOp::Add => lhs.wrapping_add(rhs),
            ArithOp::Subtract => lhs.wrapping_sub(rhs),
            ArithOp::Multiply => lhs.wrapping_mul(rhs),
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
pub(super) enum ComparOp {
    LessThan,
    LessOrEqual,
    GreaterThan,
    GreaterOrEqual,
    Equal,
    NotEqual,
}

impl ComparOp {
    pub(super) fn eval(self, lhs: i64, rhs: i64) -> bool {
        match self {
            ComparOp::LessThan => lhs < rhs,
            ComparOp::LessOrEqual => lhs <= rhs,
            ComparOp::GreaterThan => lhs > rhs,
            ComparOp::GreaterOrEqual => lhs >= rhs,
            ComparOp::Equal => lhs == rhs,
            ComparOp::NotEqual => lhs != rhs,
        }
    }

    pub(super) fn to_condition_source(self, lhs: &str, rhs: &str) -> String {
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
