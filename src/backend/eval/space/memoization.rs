//! Memoization operations.
//!
//! This module handles memoization tables and caching:
//! - new-memo: Create a new memoization table
//! - memo: Memoized evaluation (caches all results)
//! - memo-first: Memoized evaluation (caches only first result)
//! - clear-memo!: Clear all cached entries
//! - memo-stats: Get cache statistics

use crate::backend::environment::Environment;
use crate::backend::models::{MettaValue, MettaValueInner};

use super::super::{EvalStep, MemoOpType};

/// Step version of eval_new_memo - defers evaluation to trampoline.
/// Usage: (new-memo "name") or (new-memo "name" max-size)
pub(crate) fn eval_new_memo_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    // Allow 1 or 2 arguments: name [max-size]
    if items.len() < 2 || items.len() > 3 {
        let err = MettaValue::Error(
            "new-memo: requires 1 or 2 arguments. Usage: (new-memo \"name\") or (new-memo \"name\" max-size)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    let name_arg = items[1].clone();
    let size_arg = if items.len() == 3 {
        Some(items[2].clone())
    } else {
        None
    };

    EvalStep::StartNewMemo {
        name_arg,
        size_arg,
        env,
        depth,
    }
}

/// Step version of eval_memo - defers evaluation to trampoline.
/// Usage: (memo memo-table expr)
pub(crate) fn eval_memo_step(items: Vec<MettaValue>, env: Environment, depth: usize) -> EvalStep {
    if items.len() < 3 {
        let err = MettaValue::Error(
            "memo requires 2 arguments. Usage: (memo memo-table expr)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    EvalStep::StartMemo {
        memo_ref: items[1].clone(),
        expr: items[2].clone(),
        first_only: false,
        env,
        depth,
    }
}

/// Step version of eval_memo_first - defers evaluation to trampoline.
/// Usage: (memo-first memo-table expr)
pub(crate) fn eval_memo_first_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 3 {
        let err = MettaValue::Error(
            "memo-first requires 2 arguments. Usage: (memo-first memo-table expr)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    EvalStep::StartMemo {
        memo_ref: items[1].clone(),
        expr: items[2].clone(),
        first_only: true,
        env,
        depth,
    }
}

/// Step version of eval_clear_memo - defers evaluation to trampoline.
/// Usage: (clear-memo! memo-table)
pub(crate) fn eval_clear_memo_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            "clear-memo! requires 1 argument. Usage: (clear-memo! memo-table)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    EvalStep::StartMemoOp {
        memo_ref: items[1].clone(),
        op_type: MemoOpType::Clear,
        env,
        depth,
    }
}

/// Step version of eval_memo_stats - defers evaluation to trampoline.
/// Usage: (memo-stats memo-table)
pub(crate) fn eval_memo_stats_step(
    items: Vec<MettaValue>,
    env: Environment,
    depth: usize,
) -> EvalStep {
    if items.len() < 2 {
        let err = MettaValue::Error(
            "memo-stats requires 1 argument. Usage: (memo-stats memo-table)".to_string(),
            MettaValue::SExpr(items),
        );
        return EvalStep::Done((vec![err], env));
    }

    EvalStep::StartMemoOp {
        memo_ref: items[1].clone(),
        op_type: MemoOpType::Stats,
        env,
        depth,
    }
}

