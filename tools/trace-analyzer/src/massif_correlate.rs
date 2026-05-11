// massif_correlate.rs — Cross-validate MeTTa trace with Valgrind massif memory profile.
//
// Compares per-category allocation estimates from the trace (GcSafepoint events +
// allocation proxies) with actual peak heap measurements from massif.

use std::collections::HashMap;

use trace_format::TraceEventKind;

use crate::function_map::{
    classify_rust_function, classify_trace_event, TraceCategory, ALL_CATEGORIES,
};
use crate::massif_parser::{
    format_bytes, parse_massif_output, walk_peak_allocations, MassifProfile,
};
use crate::reader::TraceReader;
use crate::util::{extract_operator_name, format_pct};

/// Per-operator allocation estimate from trace events.
struct OperatorAllocProfile {
    /// Count of rule applications by this operator.
    rule_applications: u64,
    /// Count of nondeterministic forks.
    nondeterministic_forks: u64,
    /// Sum of allocation_delta_bytes from GcSafepoint events attributed to this operator.
    gc_alloc_bytes: u64,
    /// Allocation proxy score (weighted combination of the above).
    alloc_estimate: u64,
    /// Duration breakdown per category (for proportional distribution).
    category_alloc: HashMap<TraceCategory, u64>,
}

impl OperatorAllocProfile {
    fn new() -> Self {
        OperatorAllocProfile {
            rule_applications: 0,
            nondeterministic_forks: 0,
            gc_alloc_bytes: 0,
            alloc_estimate: 0,
            category_alloc: HashMap::new(),
        }
    }

    fn finalize(&mut self) {
        // Weighted allocation proxy:
        //   gc_alloc_bytes (direct) + rule_applications * 128 (avg binding alloc) + forks * 256
        self.alloc_estimate =
            self.gc_alloc_bytes + self.rule_applications * 128 + self.nondeterministic_forks * 256;
    }
}

/// Per-thread depth tracker for attributing GcSafepoint events to the most recent operator.
struct DepthTracker {
    /// Stack of (head_symbol, depth) — most recent operator context.
    stack: Vec<(String, u32)>,
}

impl DepthTracker {
    fn new() -> Self {
        DepthTracker { stack: Vec::new() }
    }

    /// Update the stack when we see an event at a given depth.
    fn track(&mut self, head: &str, depth: u32) {
        // Pop entries deeper than or equal to current depth
        while let Some((_, d)) = self.stack.last() {
            if *d >= depth {
                self.stack.pop();
            } else {
                break;
            }
        }
        self.stack.push((head.to_string(), depth));
    }

    /// Get the most recent head symbol (current operator context).
    fn current_head(&self) -> Option<&str> {
        self.stack.last().map(|(h, _)| h.as_str())
    }
}

/// Operator memory attribution estimate.
struct OperatorMemEstimate {
    operator: String,
    alloc_estimate: u64,
    rule_applications: u64,
    forks: u64,
    gc_alloc_bytes: u64,
    estimated_mem_pct: f64,
}

pub fn run(file: &str, massif_path: &str, top_n: usize, json: bool) -> Result<(), String> {
    // 1. Build MeTTa trace allocation profile (single pass)
    let reader = TraceReader::open(file)?;
    let mut op_profiles: HashMap<String, OperatorAllocProfile> = HashMap::new();
    let mut category_alloc: HashMap<TraceCategory, u64> = HashMap::new();
    let mut depth_trackers: HashMap<u32, DepthTracker> = HashMap::new();
    let mut total_events: u64 = 0;
    let mut total_gc_bytes: u64 = 0;
    let mut gc_safepoint_count: u64 = 0;

    for event in reader.events() {
        total_events += 1;

        let cat = classify_trace_event(&event.kind);
        let head = extract_operator_name(&event.input, &event.kind);

        // Track depth for GcSafepoint attribution
        let tracker = depth_trackers
            .entry(event.thread_id)
            .or_insert_with(DepthTracker::new);
        tracker.track(&head, event.depth);

        match &event.kind {
            TraceEventKind::GcSafepoint {
                allocation_delta_bytes,
                ..
            } => {
                gc_safepoint_count += 1;
                let bytes = *allocation_delta_bytes;
                total_gc_bytes += bytes;

                // Attribute to current operator context
                let attr_head = tracker.current_head().unwrap_or(&head).to_string();
                let profile = op_profiles
                    .entry(attr_head)
                    .or_insert_with(OperatorAllocProfile::new);
                profile.gc_alloc_bytes += bytes;
                *profile
                    .category_alloc
                    .entry(TraceCategory::GarbageCollection)
                    .or_insert(0) += bytes;

                *category_alloc
                    .entry(TraceCategory::GarbageCollection)
                    .or_insert(0) += bytes;
            }

            TraceEventKind::RuleApplication { .. } => {
                let profile = op_profiles
                    .entry(head.clone())
                    .or_insert_with(OperatorAllocProfile::new);
                profile.rule_applications += 1;
                *profile
                    .category_alloc
                    .entry(TraceCategory::RuleMatching)
                    .or_insert(0) += 128; // proxy

                *category_alloc.entry(cat).or_insert(0) += 128;
            }

            TraceEventKind::NondeterministicFork { .. } => {
                let profile = op_profiles
                    .entry(head.clone())
                    .or_insert_with(OperatorAllocProfile::new);
                profile.nondeterministic_forks += 1;
                *profile
                    .category_alloc
                    .entry(TraceCategory::Nondeterminism)
                    .or_insert(0) += 256; // proxy

                *category_alloc.entry(cat).or_insert(0) += 256;
            }

            TraceEventKind::PatternMatch { bindings, .. } => {
                let binding_bytes = bindings.len() as u64 * 64; // proxy per binding
                let profile = op_profiles
                    .entry(head.clone())
                    .or_insert_with(OperatorAllocProfile::new);
                *profile
                    .category_alloc
                    .entry(TraceCategory::PatternBinding)
                    .or_insert(0) += binding_bytes;

                *category_alloc.entry(cat).or_insert(0) += binding_bytes;
            }

            _ => {
                // For other timed events, use duration as a rough allocation proxy
                if let Some(duration) = event.duration_ns {
                    // 1 byte per 100ns as a rough proxy (very approximate)
                    let proxy = duration / 100;
                    if proxy > 0 {
                        let profile = op_profiles
                            .entry(head.clone())
                            .or_insert_with(OperatorAllocProfile::new);
                        *profile.category_alloc.entry(cat).or_insert(0) += proxy;
                        *category_alloc.entry(cat).or_insert(0) += proxy;
                    }
                }
            }
        }
    }

    // Finalize operator profiles
    for profile in op_profiles.values_mut() {
        profile.finalize();
    }

    // 2. Parse massif profile
    let massif_profile = parse_massif_output(massif_path)?;

    // 3. Classify massif allocations into categories
    let peak_allocs = walk_peak_allocations(&massif_profile);
    let mut massif_category_bytes: HashMap<TraceCategory, u64> = HashMap::new();
    for (func_name, bytes, _pct) in &peak_allocs {
        let cat = classify_rust_function(func_name);
        *massif_category_bytes.entry(cat).or_insert(0) += bytes;
    }

    let peak_bytes = massif_profile
        .peak_snapshot_idx
        .map(|i| massif_profile.snapshots[i].mem_heap_bytes)
        .unwrap_or(0);

    // 4. Category-level comparison
    let total_category_alloc: u64 = category_alloc.values().sum::<u64>().max(1);
    let peak_bytes_f = peak_bytes.max(1) as f64;

    // 5. Operator-level memory attribution (heuristic)
    let mut op_estimates: Vec<OperatorMemEstimate> = Vec::new();
    for (op, profile) in &op_profiles {
        let mut estimated_pct = 0.0;
        for (&cat, &alloc) in &profile.category_alloc {
            let cat_total = *category_alloc.get(&cat).unwrap_or(&1);
            if cat_total == 0 {
                continue;
            }
            let cat_massif_pct =
                *massif_category_bytes.get(&cat).unwrap_or(&0) as f64 / peak_bytes_f * 100.0;
            estimated_pct += (alloc as f64 / cat_total as f64) * cat_massif_pct;
        }
        op_estimates.push(OperatorMemEstimate {
            operator: op.clone(),
            alloc_estimate: profile.alloc_estimate,
            rule_applications: profile.rule_applications,
            forks: profile.nondeterministic_forks,
            gc_alloc_bytes: profile.gc_alloc_bytes,
            estimated_mem_pct: estimated_pct,
        });
    }
    op_estimates.sort_by(|a, b| {
        b.estimated_mem_pct
            .partial_cmp(&a.estimated_mem_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 6. Output
    if json {
        print_json(
            &category_alloc,
            &massif_category_bytes,
            &op_estimates,
            &massif_profile,
            &peak_allocs,
            peak_bytes,
            total_category_alloc,
            top_n,
        );
    } else {
        print_text(
            &category_alloc,
            &massif_category_bytes,
            &op_estimates,
            &massif_profile,
            &peak_allocs,
            peak_bytes,
            total_category_alloc,
            total_events,
            gc_safepoint_count,
            total_gc_bytes,
            top_n,
        );
    }

    Ok(())
}

fn print_text(
    category_alloc: &HashMap<TraceCategory, u64>,
    massif_category_bytes: &HashMap<TraceCategory, u64>,
    op_estimates: &[OperatorMemEstimate],
    massif_profile: &MassifProfile,
    peak_allocs: &[(String, u64, f64)],
    peak_bytes: u64,
    total_category_alloc: u64,
    total_events: u64,
    gc_safepoint_count: u64,
    total_gc_bytes: u64,
    top_n: usize,
) {
    println!("=== Massif Memory Cross-Validation Report ===");
    println!();

    // Peak heap summary
    println!("--- Peak Heap Summary ---");
    println!();
    println!("Command: {}", massif_profile.command);
    println!("Time unit: {}", massif_profile.time_unit);
    println!("Total snapshots: {}", massif_profile.snapshots.len());
    if let Some(idx) = massif_profile.peak_snapshot_idx {
        let snap = &massif_profile.snapshots[idx];
        println!(
            "Peak snapshot: #{} at time {} — {} heap + {} extra",
            snap.snapshot_num,
            snap.time,
            format_bytes(snap.mem_heap_bytes),
            format_bytes(snap.mem_heap_extra_bytes),
        );
    }
    println!();
    println!(
        "Trace: {} events, {} GC safepoints, {} tracked alloc bytes",
        total_events,
        gc_safepoint_count,
        format_bytes(total_gc_bytes),
    );

    // Category comparison
    println!();
    println!("--- Category-Level Memory Comparison ---");
    println!();
    println!(
        "{:<20} {:>14} {:>8} {:>14} {:>8}",
        "Category", "Trace Est.", "Trace%", "Massif", "Massif%"
    );
    println!("{}", "-".repeat(68));

    for &cat in &ALL_CATEGORIES {
        let t_alloc = *category_alloc.get(&cat).unwrap_or(&0);
        let m_bytes = *massif_category_bytes.get(&cat).unwrap_or(&0);
        if t_alloc == 0 && m_bytes == 0 {
            continue;
        }
        let t_pct = t_alloc as f64 / total_category_alloc as f64;
        let m_pct = m_bytes as f64 / peak_bytes.max(1) as f64;

        println!(
            "{:<20} {:>14} {:>8} {:>14} {:>8}",
            format!("{}", cat),
            format_bytes(t_alloc),
            format_pct(t_pct),
            format_bytes(m_bytes),
            format_pct(m_pct),
        );
    }

    // Note about Allocation category
    println!();
    println!(
        "Note: '{}' category appears large in massif but invisible in traces — ",
        TraceCategory::Allocation
    );
    println!("this is expected (slab allocator infrastructure overhead).");

    // Massif allocation hotspots
    if !peak_allocs.is_empty() {
        println!();
        println!("--- Massif Peak Allocation Hotspots ---");
        println!();
        println!(
            "{:<40} {:>12} {:>8} {:>16}",
            "Function", "Bytes", "Peak%", "Category"
        );
        println!("{}", "-".repeat(80));

        for (func, bytes, pct) in peak_allocs.iter().take(top_n) {
            let cat = classify_rust_function(func);
            let name_display = if func.len() > 40 {
                format!("{}...", &func[..37])
            } else {
                func.clone()
            };
            println!(
                "{:<40} {:>12} {:>8} {:>16}",
                name_display,
                format_bytes(*bytes),
                format!("{:.1}%", pct),
                format!("{}", cat),
            );
        }
    }

    // Top operators by estimated memory impact
    println!();
    println!("--- Top Operators by Estimated Memory Impact (heuristic) ---");
    println!();
    println!(
        "{:<30} {:>8} {:>8} {:>12} {:>10}",
        "Operator", "Rules", "Forks", "GC Bytes", "Est Mem%"
    );
    println!("{}", "-".repeat(72));

    for est in op_estimates.iter().take(top_n) {
        let op_display = if est.operator.len() > 30 {
            format!("{}...", &est.operator[..27])
        } else {
            est.operator.clone()
        };
        println!(
            "{:<30} {:>8} {:>8} {:>12} {:>9}",
            op_display,
            est.rule_applications,
            est.forks,
            format_bytes(est.gc_alloc_bytes),
            format!("{:.1}%", est.estimated_mem_pct),
        );
    }

    // Memory growth timeline
    if massif_profile.snapshots.len() > 1 {
        println!();
        println!("--- Memory Growth Timeline ---");
        println!();
        println!("{:>4} {:>12} {:>12} {:>12}", "#", "Time", "Heap", "Extra");
        println!("{}", "-".repeat(44));

        // Show at most 20 evenly-spaced snapshots
        let total = massif_profile.snapshots.len();
        let step = if total > 20 { total / 20 } else { 1 };
        for (i, snap) in massif_profile.snapshots.iter().enumerate() {
            if i % step == 0 || i == total - 1 {
                println!(
                    "{:>4} {:>12} {:>12} {:>12}",
                    snap.snapshot_num,
                    snap.time,
                    format_bytes(snap.mem_heap_bytes),
                    format_bytes(snap.mem_heap_extra_bytes),
                );
            }
        }
    }
}

fn print_json(
    category_alloc: &HashMap<TraceCategory, u64>,
    massif_category_bytes: &HashMap<TraceCategory, u64>,
    op_estimates: &[OperatorMemEstimate],
    massif_profile: &MassifProfile,
    peak_allocs: &[(String, u64, f64)],
    peak_bytes: u64,
    total_category_alloc: u64,
    top_n: usize,
) {
    let peak_bytes_f = peak_bytes.max(1) as f64;

    println!("{{");
    println!("  \"peak_heap_bytes\": {},", peak_bytes);
    println!("  \"command\": {:?},", massif_profile.command);
    println!("  \"snapshot_count\": {},", massif_profile.snapshots.len());

    // Categories
    println!("  \"categories\": [");
    let mut active_cats: Vec<TraceCategory> = Vec::new();
    for &cat in &ALL_CATEGORIES {
        let t = *category_alloc.get(&cat).unwrap_or(&0);
        let m = *massif_category_bytes.get(&cat).unwrap_or(&0);
        if t > 0 || m > 0 {
            active_cats.push(cat);
        }
    }
    for (i, &cat) in active_cats.iter().enumerate() {
        let t = *category_alloc.get(&cat).unwrap_or(&0);
        let m = *massif_category_bytes.get(&cat).unwrap_or(&0);
        let t_pct = t as f64 / total_category_alloc as f64 * 100.0;
        let m_pct = m as f64 / peak_bytes_f * 100.0;
        let comma = if i + 1 < active_cats.len() { "," } else { "" };
        println!(
            "    {{\"category\": \"{}\", \"trace_est_bytes\": {}, \"trace_pct\": {:.2}, \"massif_bytes\": {}, \"massif_pct\": {:.2}}}{}",
            cat, t, t_pct, m, m_pct, comma
        );
    }
    println!("  ],");

    // Peak allocations
    println!("  \"peak_allocations\": [");
    let allocs: Vec<_> = peak_allocs.iter().take(top_n).collect();
    for (i, (func, bytes, pct)) in allocs.iter().enumerate() {
        let cat = classify_rust_function(func);
        let comma = if i + 1 < allocs.len() { "," } else { "" };
        println!(
            "    {{\"function\": {:?}, \"bytes\": {}, \"peak_pct\": {:.2}, \"category\": \"{}\"}}{}",
            func, bytes, pct, cat, comma
        );
    }
    println!("  ],");

    // Top operators
    println!("  \"top_operators\": [");
    let ops: Vec<_> = op_estimates.iter().take(top_n).collect();
    for (i, est) in ops.iter().enumerate() {
        let comma = if i + 1 < ops.len() { "," } else { "" };
        println!(
            "    {{\"operator\": {:?}, \"rules\": {}, \"forks\": {}, \"gc_bytes\": {}, \"estimated_mem_pct\": {:.2}}}{}",
            est.operator, est.rule_applications, est.forks, est.gc_alloc_bytes, est.estimated_mem_pct, comma
        );
    }
    println!("  ],");

    // Timeline
    println!("  \"timeline\": [");
    let snaps = &massif_profile.snapshots;
    for (i, snap) in snaps.iter().enumerate() {
        let comma = if i + 1 < snaps.len() { "," } else { "" };
        println!(
            "    {{\"snapshot\": {}, \"time\": {}, \"heap_bytes\": {}, \"extra_bytes\": {}}}{}",
            snap.snapshot_num, snap.time, snap.mem_heap_bytes, snap.mem_heap_extra_bytes, comma
        );
    }
    println!("  ]");

    println!("}}");
}
