//! Dependency closure analysis.
//!
//! Walks the trace event stream and computes the transitive closure of head
//! symbols invoked during evaluation — the set of every operator (kernel op,
//! special form, grounded op, or user-defined rule head) reached from the
//! top-level program. Used to determine which built-ins a benchmark
//! transitively depends on, so each one can be audited against the MeTTa
//! specification.
//!
//! Output:
//! 1. Total distinct head symbols.
//! 2. Per-category listing (kernel ops / special forms / pseudo / other) with
//!    invocation counts.
//! 3. Call graph: for each parent operator, the top-N children it invoked
//!    (parent inferred from the per-thread depth stack).
//!
//! Algorithm: O(N) time, O(K) space (K = distinct head symbols). For each
//! timed event, derive the operator name via `extract_operator_name`. Pop the
//! per-thread stack down to the event's depth, attribute an edge from the
//! current top (parent) to the new operator, then push the new operator.

use std::collections::{BTreeMap, HashMap};

use crate::reader::TraceReader;
use crate::util::extract_operator_name;

/// Categorize a head symbol per spec §6.3.6 (kernel ops) and MeTTaTron's
/// known stdlib special forms.
fn categorize(name: &str) -> &'static str {
    match name {
        // Spec §6.3.6 — embedded kernel ops
        "eval" | "evalc" | "chain" | "unify" | "cons-atom" | "decons-atom"
        | "function" | "return" | "collapse-bind" | "superpose-bind"
        | "metta" | "call-native" | "context-space" => "kernel-op",
        // Stdlib control flow / binding forms (spec §11)
        "let" | "let*" | "if" | "case" | "switch" | "match" | "match-or"
        | "match-atom" | "sealed" | "atom-subst" => "special-form",
        // Pseudo-events emitted by extract_operator_name
        s if s.starts_with('<') => "pseudo-event",
        // Everything else: grounded op, stdlib reducible, or user-defined rule head
        _ => "operator",
    }
}

pub fn run(file: &str, top_n: usize, show_graph: bool) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    // Per-thread stack of (depth, operator_name) for parent attribution.
    let mut thread_stacks: HashMap<u32, Vec<(u32, String)>> = HashMap::new();

    let mut all_ops: BTreeMap<String, u64> = BTreeMap::new();
    let mut edges: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    let mut total_events: u64 = 0;

    for event in reader.events() {
        total_events += 1;
        let op = extract_operator_name(&event.input, &event.kind);
        if op.is_empty() {
            continue;
        }

        *all_ops.entry(op.clone()).or_insert(0) += 1;

        let stack = thread_stacks.entry(event.thread_id).or_default();
        // Pop frames whose depth is ≥ current event's depth (siblings or older).
        while let Some((d, _)) = stack.last() {
            if *d >= event.depth {
                stack.pop();
            } else {
                break;
            }
        }
        if let Some((_, parent)) = stack.last() {
            *edges
                .entry(parent.clone())
                .or_default()
                .entry(op.clone())
                .or_insert(0) += 1;
        }
        stack.push((event.depth, op));
    }

    println!("=== Dependency Closure ===");
    println!("Total events processed: {}", total_events);
    println!("Distinct head symbols:  {}", all_ops.len());
    println!();

    // Categorize
    let mut by_category: BTreeMap<&'static str, Vec<(&str, u64)>> = BTreeMap::new();
    for (op, count) in &all_ops {
        by_category
            .entry(categorize(op))
            .or_default()
            .push((op.as_str(), *count));
    }

    // Sort each category by count descending
    for entries in by_category.values_mut() {
        entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    }

    // Print categories in a fixed sensible order
    for cat in &["kernel-op", "special-form", "operator", "pseudo-event"] {
        if let Some(entries) = by_category.get(*cat) {
            println!("--- {} ({} ops, {} total invocations) ---",
                cat,
                entries.len(),
                entries.iter().map(|(_, c)| *c).sum::<u64>()
            );
            for (op, count) in entries {
                println!("  {:>10}× {}", count, op);
            }
            println!();
        }
    }

    if show_graph {
        println!("=== Call Graph (top {} children per parent) ===", top_n);
        for (parent, children) in &edges {
            let mut sorted: Vec<_> = children.iter().collect();
            sorted.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
            let total: u64 = children.values().sum();
            println!("{} ({} edges, {} total invocations):", parent, children.len(), total);
            for (child, count) in sorted.iter().take(top_n) {
                println!("  → {:>10}× {}", count, child);
            }
            if children.len() > top_n {
                println!("  → ... ({} more children omitted)", children.len() - top_n);
            }
        }
    }

    Ok(())
}
