//! Memoization operations.
//!
//! This module handles memoization tables and caching:
//! - new-memo: Create a new memoization table
//! - memo: Memoized evaluation (caches all results)
//! - memo-first: Memoized evaluation (caches only first result)
//! - clear-memo!: Clear all cached entries
//! - memo-stats: Get cache statistics

use crate::backend::environment::HeapEnvironment;
use crate::backend::models::{EvalResult, MemoHandle, MettaValue, MettaValueInner};

#[allow(unused_imports)]
use super::super::eval;
use super::super::{EvalStep, MemoOpType};

/// Step version of eval_new_memo - defers evaluation to trampoline.
/// Usage: (new-memo "name") or (new-memo "name" max-size)
pub(crate) fn eval_new_memo_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
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

/// new-memo: Create a new memoization table
/// Usage: (new-memo "name")
/// Optional: (new-memo "name" max-size) for LRU eviction
/// Returns a Memo handle that can be used with memo/memo-first
///
/// DEPRECATED: Use eval_new_memo_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_new_memo(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    // Allow 1 or 2 arguments: name [max-size]
    if items.len() < 2 || items.len() > 3 {
        let err = MettaValue::Error(
            "new-memo: requires 1 or 2 arguments. Usage: (new-memo \"name\") or (new-memo \"name\" max-size)".to_string(),
            MettaValue::SExpr(items),
        );
        return (vec![err], env);
    }

    let name_arg = &items[1];

    // Evaluate the name argument
    let (name_results, env1) = eval(name_arg.clone(), env);
    if name_results.is_empty() {
        let err = MettaValue::Error(
            "new-memo: name evaluated to empty".to_string(),
            name_arg.clone(),
        );
        return (vec![err], env1);
    }

    // Extract string name
    let name = match name_results[0].inner() {
        MettaValueInner::String(s) => s.clone(),
        MettaValueInner::Atom(s) => s.clone(),
        _ => {
            let err = MettaValue::Error(
                format!(
                    "new-memo: name must be a string or atom, got {}",
                    super::super::friendly_value_repr(&name_results[0])
                ),
                name_results[0].clone(),
            );
            return (vec![err], env1);
        }
    };

    // Check for optional max-size argument
    if items.len() == 3 {
        let size_arg = &items[2];
        let (size_results, env2) = eval(size_arg.clone(), env1);
        if size_results.is_empty() {
            let err = MettaValue::Error(
                "new-memo: max-size evaluated to empty".to_string(),
                size_arg.clone(),
            );
            return (vec![err], env2);
        }

        match size_results[0].inner() {
            MettaValueInner::Long(n) if *n > 0 => {
                let memo = MemoHandle::with_max_size(name, *n as usize);
                (vec![MettaValue::Memo(memo)], env2)
            }
            _ => {
                let err = MettaValue::Error(
                    format!(
                        "new-memo: max-size must be a positive integer, got {}",
                        super::super::friendly_value_repr(&size_results[0])
                    ),
                    size_results[0].clone(),
                );
                (vec![err], env2)
            }
        }
    } else {
        // No max-size - unlimited cache
        let memo = MemoHandle::new(name);
        (vec![MettaValue::Memo(memo)], env1)
    }
}

/// Step version of eval_memo - defers evaluation to trampoline.
/// Usage: (memo memo-table expr)
pub(crate) fn eval_memo_step(items: Vec<MettaValue>, env: HeapEnvironment, depth: usize) -> EvalStep {
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

/// memo: Memoized evaluation - caches all results
/// Usage: (memo memo-table expr)
/// Returns cached results if available, otherwise evaluates expr and caches results
///
/// DEPRECATED: Use eval_memo_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_memo(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("memo", items, 2, env, "(memo memo-table expr)");

    let memo_ref = &items[1];
    let expr = &items[2];

    // Evaluate the memo reference
    let (memo_results, env1) = eval(memo_ref.clone(), env);
    if memo_results.is_empty() {
        let err = MettaValue::Error(
            "memo: memo-table evaluated to empty".to_string(),
            memo_ref.clone(),
        );
        return (vec![err], env1);
    }

    match memo_results[0].inner() {
        MettaValueInner::Memo(handle) => {
            // Check cache first
            if let Some(cached) = handle.lookup(expr) {
                return (cached, env1);
            }

            // Not cached - evaluate and store
            let (results, env2) = eval(expr.clone(), env1);

            // Store in cache (only if non-empty)
            if !results.is_empty() {
                handle.store(expr, results.clone(), false);
            }

            (results, env2)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "memo: first argument must be a memo table, got {}. Usage: (memo memo-table expr)",
                    super::super::friendly_value_repr(&memo_results[0])
                ),
                memo_results[0].clone(),
            );
            (vec![err], env1)
        }
    }
}

/// Step version of eval_memo_first - defers evaluation to trampoline.
/// Usage: (memo-first memo-table expr)
pub(crate) fn eval_memo_first_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
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

/// memo-first: Memoized evaluation - caches only first result
/// Usage: (memo-first memo-table expr)
/// Returns cached first result if available, otherwise evaluates expr and caches first result
/// Useful for deterministic/backtracking scenarios where only one result is needed
///
/// DEPRECATED: Use eval_memo_first_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_memo_first(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("memo-first", items, 2, env, "(memo-first memo-table expr)");

    let memo_ref = &items[1];
    let expr = &items[2];

    // Evaluate the memo reference
    let (memo_results, env1) = eval(memo_ref.clone(), env);
    if memo_results.is_empty() {
        let err = MettaValue::Error(
            "memo-first: memo-table evaluated to empty".to_string(),
            memo_ref.clone(),
        );
        return (vec![err], env1);
    }

    match memo_results[0].inner() {
        MettaValueInner::Memo(handle) => {
            // Check cache first
            if let Some(cached) = handle.lookup(expr) {
                return (cached, env1);
            }

            // Not cached - evaluate and store only first result
            let (results, env2) = eval(expr.clone(), env1);

            // Store only first result in cache
            if !results.is_empty() {
                handle.store(expr, vec![results[0].clone()], true);
            }

            (results, env2)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "memo-first: first argument must be a memo table, got {}. Usage: (memo-first memo-table expr)",
                    super::super::friendly_value_repr(&memo_results[0])
                ),
                memo_results[0].clone(),
            );
            (vec![err], env1)
        }
    }
}

/// Step version of eval_clear_memo - defers evaluation to trampoline.
/// Usage: (clear-memo! memo-table)
pub(crate) fn eval_clear_memo_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
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

/// clear-memo!: Clear all cached entries from a memo table
/// Usage: (clear-memo! memo-table)
/// Returns the memo table for chaining
///
/// DEPRECATED: Use eval_clear_memo_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_clear_memo(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("clear-memo!", items, 1, env, "(clear-memo! memo-table)");

    let memo_ref = &items[1];

    // Evaluate the memo reference
    let (memo_results, env1) = eval(memo_ref.clone(), env);
    if memo_results.is_empty() {
        let err = MettaValue::Error(
            "clear-memo!: memo-table evaluated to empty".to_string(),
            memo_ref.clone(),
        );
        return (vec![err], env1);
    }

    match memo_results[0].inner() {
        MettaValueInner::Memo(handle) => {
            handle.clear();
            (vec![MettaValue::Memo(handle.clone())], env1)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "clear-memo!: argument must be a memo table, got {}. Usage: (clear-memo! memo-table)",
                    super::super::friendly_value_repr(&memo_results[0])
                ),
                memo_results[0].clone(),
            );
            (vec![err], env1)
        }
    }
}

/// Step version of eval_memo_stats - defers evaluation to trampoline.
/// Usage: (memo-stats memo-table)
pub(crate) fn eval_memo_stats_step(
    items: Vec<MettaValue>,
    env: HeapEnvironment,
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

/// memo-stats: Get statistics about a memo table
/// Usage: (memo-stats memo-table)
/// Returns (stats hits misses size max-size hit-rate)
///
/// DEPRECATED: Use eval_memo_stats_step for trampoline-based evaluation.
#[allow(dead_code)]
pub(crate) fn eval_memo_stats(items: Vec<MettaValue>, env: HeapEnvironment) -> EvalResult {
    require_args_with_usage!("memo-stats", items, 1, env, "(memo-stats memo-table)");

    let memo_ref = &items[1];

    // Evaluate the memo reference
    let (memo_results, env1) = eval(memo_ref.clone(), env);
    if memo_results.is_empty() {
        let err = MettaValue::Error(
            "memo-stats: memo-table evaluated to empty".to_string(),
            memo_ref.clone(),
        );
        return (vec![err], env1);
    }

    match memo_results[0].inner() {
        MettaValueInner::Memo(handle) => {
            let (hits, misses, size, max_size) = handle.stats();
            let hit_rate = handle.hit_rate();

            // Return as S-expression: (stats hits misses size max-size hit-rate%)
            let stats = MettaValue::SExpr(vec![
                MettaValue::Atom("stats".to_string()),
                MettaValue::Long(hits as i64),
                MettaValue::Long(misses as i64),
                MettaValue::Long(size as i64),
                MettaValue::Long(max_size as i64),
                MettaValue::Float(hit_rate),
            ]);
            (vec![stats], env1)
        }
        _ => {
            let err = MettaValue::Error(
                format!(
                    "memo-stats: argument must be a memo table, got {}. Usage: (memo-stats memo-table)",
                    super::super::friendly_value_repr(&memo_results[0])
                ),
                memo_results[0].clone(),
            );
            (vec![err], env1)
        }
    }
}
