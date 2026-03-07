//! Nondeterministic branching deep-dive analysis.
//!
//! Comprehensive fork/branch analysis:
//! - Branch count distribution (histogram of branches per fork)
//! - Branch productivity: % of BranchEnd with result_count == 0 (wasted speculation)
//! - Cumulative wasted time from empty branches
//! - Fork nesting depth (forks within branches of other forks) — combinatorial explosion detection
//! - Per-fork-point grouping by source location
//!
//! Algorithm: Single-pass. Per-thread fork stacks for nesting tracking.

use std::collections::HashMap;

use trace_format::TraceEventKind;

use crate::reader::TraceReader;
use crate::util::{extract_operator_name, format_duration_ns};

/// Record of a nondeterministic fork event.
struct ForkRecord {
    timestamp_ns: u64,
    branch_count: u32,
    depth: u32,
    thread_id: u32,
    nesting_depth: u32,
    head_symbol: String,
}

/// Record of a completed branch.
struct BranchRecord {
    result_count: u32,
    duration_ns: u64,
    _branch_index: u32,
}

/// Per-thread fork nesting tracker.
struct ThreadForkStack {
    /// Stack of active fork depths (for nesting detection).
    fork_depths: Vec<u32>,
}

impl ThreadForkStack {
    fn new() -> Self {
        Self {
            fork_depths: Vec::new(),
        }
    }

    /// Enter a fork: push depth, return nesting level.
    fn enter_fork(&mut self, depth: u32) -> u32 {
        let nesting = self.fork_depths.len() as u32;
        self.fork_depths.push(depth);
        nesting
    }

    /// Exit a fork at or above the given depth.
    fn exit_to_depth(&mut self, depth: u32) {
        while let Some(&top_depth) = self.fork_depths.last() {
            if top_depth >= depth {
                self.fork_depths.pop();
            } else {
                break;
            }
        }
    }
}

pub fn run(file: &str, top_n: usize) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    let mut forks: Vec<ForkRecord> = Vec::new();
    let mut branches: Vec<BranchRecord> = Vec::new();
    let mut thread_stacks: HashMap<u32, ThreadForkStack> = HashMap::new();
    let mut total_events: u64 = 0;
    let mut total_wall_ns: u64 = 0;

    // Per-source-location fork grouping
    let mut fork_by_head: HashMap<String, Vec<usize>> = HashMap::new();

    for event in reader.events() {
        total_events += 1;
        let end_ns = event.timestamp_ns + event.duration_ns.unwrap_or(0);
        if end_ns > total_wall_ns {
            total_wall_ns = end_ns;
        }

        match &event.kind {
            TraceEventKind::NondeterministicFork { branch_count } => {
                let stack = thread_stacks
                    .entry(event.thread_id)
                    .or_insert_with(ThreadForkStack::new);
                let nesting = stack.enter_fork(event.depth);

                let head = extract_operator_name(&event.input, &event.kind);
                let fork_idx = forks.len();
                fork_by_head
                    .entry(head.clone())
                    .or_default()
                    .push(fork_idx);

                forks.push(ForkRecord {
                    timestamp_ns: event.timestamp_ns,
                    branch_count: *branch_count,
                    depth: event.depth,
                    thread_id: event.thread_id,
                    nesting_depth: nesting,
                    head_symbol: head,
                });
            }
            TraceEventKind::BranchEnd {
                branch_index,
                result_count,
            } => {
                let duration = event.duration_ns.unwrap_or(0);
                branches.push(BranchRecord {
                    result_count: *result_count,
                    duration_ns: duration,
                    _branch_index: *branch_index,
                });

                // Pop fork stack when we return to parent depth
                if let Some(stack) = thread_stacks.get_mut(&event.thread_id) {
                    stack.exit_to_depth(event.depth);
                }
            }
            _ => {}
        }
    }

    // Compute statistics
    let total_forks = forks.len();
    let total_branches = branches.len();

    // Branch count distribution
    let mut branch_count_hist: HashMap<u32, u64> = HashMap::new();
    for fork in &forks {
        *branch_count_hist.entry(fork.branch_count).or_default() += 1;
    }

    // Branch productivity
    let empty_branches: u64 = branches.iter().filter(|b| b.result_count == 0).count() as u64;
    let empty_branch_pct = if total_branches > 0 {
        empty_branches as f64 / total_branches as f64 * 100.0
    } else {
        0.0
    };
    let wasted_time_ns: u64 = branches
        .iter()
        .filter(|b| b.result_count == 0)
        .map(|b| b.duration_ns)
        .sum();

    // Fork nesting depth distribution
    let mut nesting_hist: HashMap<u32, u64> = HashMap::new();
    let max_nesting = forks.iter().map(|f| f.nesting_depth).max().unwrap_or(0);
    for fork in &forks {
        *nesting_hist.entry(fork.nesting_depth).or_default() += 1;
    }

    // Print report
    println!("=== Fork/Branch Fanout Analysis ===");
    println!();
    println!("Source: {}", reader.header.source_file);
    println!("Total events: {total_events}");
    println!("Wall time: {}", format_duration_ns(total_wall_ns));
    println!();

    println!("--- Summary ---");
    println!("  Total forks: {total_forks}");
    println!("  Total branches: {total_branches}");
    println!("  Empty branches: {empty_branches} ({empty_branch_pct:.1}%)");
    println!("  Wasted time (empty branches): {}", format_duration_ns(wasted_time_ns));
    if total_wall_ns > 0 {
        println!(
            "  Wasted time as % of wall: {:.1}%",
            wasted_time_ns as f64 / total_wall_ns as f64 * 100.0
        );
    }
    println!("  Max fork nesting depth: {max_nesting}");
    println!();

    // Branch count distribution
    println!("--- Branch Count Distribution ---");
    let mut hist_vec: Vec<_> = branch_count_hist.into_iter().collect();
    hist_vec.sort_by_key(|&(count, _)| count);
    let max_hist_count = hist_vec.iter().map(|(_, c)| *c).max().unwrap_or(1);
    for (branch_count, occurrences) in &hist_vec {
        let bar_width = ((*occurrences as f64 / max_hist_count as f64) * 40.0) as usize;
        let bar: String = "\u{2588}".repeat(bar_width);
        println!(
            "  {:>3} branches: {:>8}  {}",
            branch_count, occurrences, bar
        );
    }
    println!();

    // Fork nesting depth
    println!("--- Fork Nesting Depth ---");
    let mut nesting_vec: Vec<_> = nesting_hist.into_iter().collect();
    nesting_vec.sort_by_key(|&(depth, _)| depth);
    let max_nesting_count = nesting_vec.iter().map(|(_, c)| *c).max().unwrap_or(1);
    for (depth, count) in &nesting_vec {
        let bar_width = ((*count as f64 / max_nesting_count as f64) * 40.0) as usize;
        let bar: String = "\u{2588}".repeat(bar_width);
        println!("  Depth {:>3}: {:>8}  {}", depth, count, bar);
    }
    println!();

    // Top fork points by total branch count
    println!("--- Top Fork Points (by total branches spawned) ---");
    let mut head_stats: Vec<_> = fork_by_head
        .iter()
        .map(|(head, indices)| {
            let total_branch_count: u32 = indices
                .iter()
                .map(|&i| forks[i].branch_count)
                .sum();
            let max_branches = indices
                .iter()
                .map(|&i| forks[i].branch_count)
                .max()
                .unwrap_or(0);
            let fork_count = indices.len();
            (head.clone(), fork_count, total_branch_count, max_branches)
        })
        .collect();

    head_stats.sort_by(|a, b| b.2.cmp(&a.2));

    println!(
        "  {:<30} {:>8} {:>12} {:>12}",
        "Head Symbol", "Forks", "Total Branches", "Max/Fork"
    );
    println!("  {}", "-".repeat(66));

    for (head, fork_count, total_branch_count, max_branches) in head_stats.iter().take(top_n) {
        let head_display = if head.len() > 30 {
            format!("{}...", &head[..27])
        } else {
            head.clone()
        };
        println!(
            "  {:<30} {:>8} {:>12} {:>12}",
            head_display, fork_count, total_branch_count, max_branches
        );
    }

    Ok(())
}
