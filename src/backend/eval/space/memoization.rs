//! Memoization operations.
//!
//! This module handles memoization tables and caching:
//! - new-memo: Create a new memoization table
//! - memo: Memoized evaluation (caches all results)
//! - memo-first: Memoized evaluation (caches only first result)
//! - clear-memo!: Clear all cached entries
//! - memo-stats: Get cache statistics

use std::sync::Arc;

use crate::backend::environment::Environment;
use crate::backend::models::{EvalResult, MemoHandle, MettaValue};

use super::super::eval;

/// new-memo: Create a new memoization table
/// Usage: (new-memo "name")
/// Optional: (new-memo "name" max-size) for LRU eviction
/// Returns a Memo handle that can be used with memo/memo-first
pub(crate) fn eval_new_memo(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    // Allow 1 or 2 arguments: name [max-size]
    if items.len() < 2 || items.len() > 3 {
        let err = MettaValue::Error(
            "new-memo: requires 1 or 2 arguments. Usage: (new-memo \"name\") or (new-memo \"name\" max-size)".to_string(),
            Arc::new(MettaValue::SExpr(items)),
        );
        return (vec![err], env);
    }

    let name_arg = &items[1];

    // Evaluate the name argument
    let (name_results, env1) = eval(name_arg.clone(), env);
    if name_results.is_empty() {
        let err = MettaValue::Error(
            "new-memo: name evaluated to empty".to_string(),
            Arc::new(name_arg.clone()),
        );
        return (vec![err], env1);
    }

    // Extract string name
    let name = match &name_results[0] {
        MettaValue::String(s) => s.clone(),
        MettaValue::Atom(s) => s.clone(),
        other => {
            let err = MettaValue::Error(
                format!(
                    "new-memo: name must be a string or atom, got {}",
                    super::super::friendly_value_repr(other)
                ),
                Arc::new(other.clone()),
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
                Arc::new(size_arg.clone()),
            );
            return (vec![err], env2);
        }

        match &size_results[0] {
            MettaValue::Long(n) if *n > 0 => {
                let memo = MemoHandle::with_max_size(name, *n as usize);
                (vec![MettaValue::Memo(memo)], env2)
            }
            other => {
                let err = MettaValue::Error(
                    format!(
                        "new-memo: max-size must be a positive integer, got {}",
                        super::super::friendly_value_repr(other)
                    ),
                    Arc::new(other.clone()),
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

/// memo: Memoized evaluation - caches all results
/// Usage: (memo memo-table expr)
/// Returns cached results if available, otherwise evaluates expr and caches results
pub(crate) fn eval_memo(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("memo", items, 2, env, "(memo memo-table expr)");

    let memo_ref = &items[1];
    let expr = &items[2];

    // Evaluate the memo reference
    let (memo_results, env1) = eval(memo_ref.clone(), env);
    if memo_results.is_empty() {
        let err = MettaValue::Error(
            "memo: memo-table evaluated to empty".to_string(),
            Arc::new(memo_ref.clone()),
        );
        return (vec![err], env1);
    }

    match &memo_results[0] {
        MettaValue::Memo(handle) => {
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
        other => {
            let err = MettaValue::Error(
                format!(
                    "memo: first argument must be a memo table, got {}. Usage: (memo memo-table expr)",
                    super::super::friendly_value_repr(other)
                ),
                Arc::new(other.clone()),
            );
            (vec![err], env1)
        }
    }
}

/// memo-first: Memoized evaluation - caches only first result
/// Usage: (memo-first memo-table expr)
/// Returns cached first result if available, otherwise evaluates expr and caches first result
/// Useful for deterministic/backtracking scenarios where only one result is needed
pub(crate) fn eval_memo_first(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("memo-first", items, 2, env, "(memo-first memo-table expr)");

    let memo_ref = &items[1];
    let expr = &items[2];

    // Evaluate the memo reference
    let (memo_results, env1) = eval(memo_ref.clone(), env);
    if memo_results.is_empty() {
        let err = MettaValue::Error(
            "memo-first: memo-table evaluated to empty".to_string(),
            Arc::new(memo_ref.clone()),
        );
        return (vec![err], env1);
    }

    match &memo_results[0] {
        MettaValue::Memo(handle) => {
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
        other => {
            let err = MettaValue::Error(
                format!(
                    "memo-first: first argument must be a memo table, got {}. Usage: (memo-first memo-table expr)",
                    super::super::friendly_value_repr(other)
                ),
                Arc::new(other.clone()),
            );
            (vec![err], env1)
        }
    }
}

/// clear-memo!: Clear all cached entries from a memo table
/// Usage: (clear-memo! memo-table)
/// Returns the memo table for chaining
pub(crate) fn eval_clear_memo(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("clear-memo!", items, 1, env, "(clear-memo! memo-table)");

    let memo_ref = &items[1];

    // Evaluate the memo reference
    let (memo_results, env1) = eval(memo_ref.clone(), env);
    if memo_results.is_empty() {
        let err = MettaValue::Error(
            "clear-memo!: memo-table evaluated to empty".to_string(),
            Arc::new(memo_ref.clone()),
        );
        return (vec![err], env1);
    }

    match &memo_results[0] {
        MettaValue::Memo(handle) => {
            handle.clear();
            (vec![MettaValue::Memo(handle.clone())], env1)
        }
        other => {
            let err = MettaValue::Error(
                format!(
                    "clear-memo!: argument must be a memo table, got {}. Usage: (clear-memo! memo-table)",
                    super::super::friendly_value_repr(other)
                ),
                Arc::new(other.clone()),
            );
            (vec![err], env1)
        }
    }
}

/// memo-stats: Get statistics about a memo table
/// Usage: (memo-stats memo-table)
/// Returns (stats hits misses size max-size hit-rate)
pub(crate) fn eval_memo_stats(items: Vec<MettaValue>, env: Environment) -> EvalResult {
    require_args_with_usage!("memo-stats", items, 1, env, "(memo-stats memo-table)");

    let memo_ref = &items[1];

    // Evaluate the memo reference
    let (memo_results, env1) = eval(memo_ref.clone(), env);
    if memo_results.is_empty() {
        let err = MettaValue::Error(
            "memo-stats: memo-table evaluated to empty".to_string(),
            Arc::new(memo_ref.clone()),
        );
        return (vec![err], env1);
    }

    match &memo_results[0] {
        MettaValue::Memo(handle) => {
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
        other => {
            let err = MettaValue::Error(
                format!(
                    "memo-stats: argument must be a memo table, got {}. Usage: (memo-stats memo-table)",
                    super::super::friendly_value_repr(other)
                ),
                Arc::new(other.clone()),
            );
            (vec![err], env1)
        }
    }
}
