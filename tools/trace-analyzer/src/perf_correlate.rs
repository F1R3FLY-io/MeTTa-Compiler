// perf_correlate.rs — Cross-validate MeTTa trace with perf CPU profile (folded stacks).
//
// Bridges the gap between native CPU profiling (perf) and MeTTa evaluation traces by
// classifying both into a shared category taxonomy, then comparing time distributions.

use std::collections::HashMap;

use crate::function_map::{
    classify_rust_function, classify_trace_event, TraceCategory, ALL_CATEGORIES,
};
use crate::perf_parser::{parse_folded_stacks, PerfProfile};
use crate::reader::TraceReader;
use crate::util::{extract_operator_name, format_duration_ns, format_pct};

/// Per-operator profiling accumulator (built from trace events).
struct OperatorProfile {
    call_count: u64,
    total_self_ns: u64,
    /// Duration breakdown per category.
    category_ns: HashMap<TraceCategory, u64>,
}

impl OperatorProfile {
    fn new() -> Self {
        OperatorProfile {
            call_count: 0,
            total_self_ns: 0,
            category_ns: HashMap::new(),
        }
    }

    fn record(&mut self, duration_ns: u64, category: TraceCategory) {
        self.call_count += 1;
        self.total_self_ns += duration_ns;
        *self.category_ns.entry(category).or_insert(0) += duration_ns;
    }
}

/// Category-level comparison row.
struct CategoryComparison {
    category: TraceCategory,
    trace_ns: u64,
    trace_pct: f64,
    perf_samples: u64,
    perf_pct: f64,
    divergence: f64, // perf_pct - trace_pct
}

/// Operator-level estimated CPU attribution.
struct OperatorCpuEstimate {
    operator: String,
    call_count: u64,
    trace_self_ns: u64,
    estimated_cpu_pct: f64,
}

pub fn run(file: &str, perf_stacks_path: &str, top_n: usize, json: bool) -> Result<(), String> {
    // 1. Build MeTTa trace profile (single pass)
    let reader = TraceReader::open(file)?;
    let mut op_profiles: HashMap<String, OperatorProfile> = HashMap::new();
    let mut category_ns: HashMap<TraceCategory, u64> = HashMap::new();
    let mut total_trace_ns: u64 = 0;
    let mut total_events: u64 = 0;
    let mut timed_events: u64 = 0;

    for event in reader.events() {
        total_events += 1;

        let duration = match event.duration_ns {
            Some(d) => d,
            None => continue,
        };
        timed_events += 1;

        let cat = classify_trace_event(&event.kind);
        *category_ns.entry(cat).or_insert(0) += duration;
        total_trace_ns += duration;

        let head = extract_operator_name(&event.input, &event.kind);
        op_profiles
            .entry(head)
            .or_insert_with(OperatorProfile::new)
            .record(duration, cat);
    }

    // 2. Build perf profile
    let perf_profile = parse_folded_stacks(perf_stacks_path)?;

    // 3. Classify perf functions into categories
    let mut perf_category_samples: HashMap<TraceCategory, u64> = HashMap::new();
    for func in &perf_profile.functions {
        let cat = classify_rust_function(&func.function_name);
        *perf_category_samples.entry(cat).or_insert(0) += func.self_samples;
    }

    // 4. Build category comparison
    let total_perf_samples = perf_profile.total_samples.max(1);
    let total_trace_ns_f = total_trace_ns.max(1) as f64;

    let mut comparisons: Vec<CategoryComparison> = Vec::new();
    for &cat in &ALL_CATEGORIES {
        let t_ns = *category_ns.get(&cat).unwrap_or(&0);
        let t_pct = t_ns as f64 / total_trace_ns_f * 100.0;
        let p_samp = *perf_category_samples.get(&cat).unwrap_or(&0);
        let p_pct = p_samp as f64 / total_perf_samples as f64 * 100.0;
        comparisons.push(CategoryComparison {
            category: cat,
            trace_ns: t_ns,
            trace_pct: t_pct,
            perf_samples: p_samp,
            perf_pct: p_pct,
            divergence: p_pct - t_pct,
        });
    }

    // 5. Pearson correlation
    let pearson = compute_pearson(&comparisons);

    // 6. Operator-level CPU attribution (heuristic)
    let mut op_estimates: Vec<OperatorCpuEstimate> = Vec::new();
    for (op, profile) in &op_profiles {
        let mut estimated_pct = 0.0;
        for (&cat, &ns) in &profile.category_ns {
            let cat_total_ns = *category_ns.get(&cat).unwrap_or(&1);
            if cat_total_ns == 0 {
                continue;
            }
            let cat_perf_pct = *perf_category_samples.get(&cat).unwrap_or(&0) as f64
                / total_perf_samples as f64
                * 100.0;
            estimated_pct += (ns as f64 / cat_total_ns as f64) * cat_perf_pct;
        }
        op_estimates.push(OperatorCpuEstimate {
            operator: op.clone(),
            call_count: profile.call_count,
            trace_self_ns: profile.total_self_ns,
            estimated_cpu_pct: estimated_pct,
        });
    }
    op_estimates.sort_by(|a, b| {
        b.estimated_cpu_pct
            .partial_cmp(&a.estimated_cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 7. Output
    if json {
        print_json(&comparisons, &op_estimates, &perf_profile, pearson, top_n);
    } else {
        print_text(
            &comparisons,
            &op_estimates,
            &perf_profile,
            pearson,
            top_n,
            total_events,
            timed_events,
            total_trace_ns,
        );
    }

    Ok(())
}

fn compute_pearson(comparisons: &[CategoryComparison]) -> f64 {
    let n = comparisons.len() as f64;
    if n < 2.0 {
        return 0.0;
    }

    let mean_t = comparisons.iter().map(|c| c.trace_pct).sum::<f64>() / n;
    let mean_p = comparisons.iter().map(|c| c.perf_pct).sum::<f64>() / n;

    let mut cov = 0.0;
    let mut var_t = 0.0;
    let mut var_p = 0.0;

    for c in comparisons {
        let dt = c.trace_pct - mean_t;
        let dp = c.perf_pct - mean_p;
        cov += dt * dp;
        var_t += dt * dt;
        var_p += dp * dp;
    }

    let denom = (var_t * var_p).sqrt();
    if denom < 1e-12 {
        0.0
    } else {
        cov / denom
    }
}

fn print_text(
    comparisons: &[CategoryComparison],
    op_estimates: &[OperatorCpuEstimate],
    perf_profile: &PerfProfile,
    pearson: f64,
    top_n: usize,
    total_events: u64,
    timed_events: u64,
    total_trace_ns: u64,
) {
    println!("=== Perf CPU Cross-Validation Report ===");
    println!();
    println!(
        "Trace: {} events ({} timed), {} total traced time",
        total_events,
        timed_events,
        format_duration_ns(total_trace_ns)
    );
    println!(
        "Perf:  {} total samples, {} unique functions",
        perf_profile.total_samples,
        perf_profile.functions.len()
    );
    println!();

    // Category comparison table
    println!("--- Category-Level Comparison ---");
    println!();
    println!(
        "{:<20} {:>10} {:>8} {:>10} {:>8} {:>10}",
        "Category", "Trace", "Trace%", "Perf", "Perf%", "Diverge"
    );
    println!("{}", "-".repeat(70));

    for c in comparisons {
        if c.trace_ns == 0 && c.perf_samples == 0 {
            continue; // skip empty categories
        }
        let diverge_marker = if c.divergence.abs() > 15.0 {
            " !!!"
        } else {
            ""
        };
        println!(
            "{:<20} {:>10} {:>8} {:>10} {:>8} {:>9}{}",
            format!("{}", c.category),
            format_duration_ns(c.trace_ns),
            format_pct(c.trace_pct / 100.0),
            c.perf_samples,
            format_pct(c.perf_pct / 100.0),
            format!("{:+.1}%", c.divergence),
            diverge_marker,
        );
    }

    println!();
    println!("Pearson correlation (trace% vs perf%): {:.3}", pearson);
    if pearson > 0.8 {
        println!("  -> Strong correlation: trace profiling aligns well with native CPU profile");
    } else if pearson > 0.5 {
        println!("  -> Moderate correlation: some discrepancy between trace and native profiles");
    } else {
        println!("  -> Weak correlation: significant gap between trace and native views");
    }

    // Divergence alerts
    let divergent: Vec<&CategoryComparison> = comparisons
        .iter()
        .filter(|c| c.divergence.abs() > 15.0)
        .collect();
    if !divergent.is_empty() {
        println!();
        println!("--- Divergence Alerts (|delta| > 15%) ---");
        for c in &divergent {
            if c.divergence > 0.0 {
                println!(
                    "  {} is {:+.1}% higher in perf than trace — native overhead not captured by trace",
                    c.category, c.divergence
                );
            } else {
                println!(
                    "  {} is {:+.1}% lower in perf than trace — trace overestimates or perf under-samples",
                    c.category, c.divergence
                );
            }
        }
    }

    // Top operators by estimated CPU
    println!();
    println!("--- Top Operators by Estimated CPU (heuristic) ---");
    println!();
    println!(
        "{:<30} {:>8} {:>12} {:>10}",
        "Operator", "Calls", "Trace Self", "Est CPU%"
    );
    println!("{}", "-".repeat(64));

    for est in op_estimates.iter().take(top_n) {
        let op_display = if est.operator.len() > 30 {
            format!("{}...", &est.operator[..27])
        } else {
            est.operator.clone()
        };
        println!(
            "{:<30} {:>8} {:>12} {:>9}",
            op_display,
            est.call_count,
            format_duration_ns(est.trace_self_ns),
            format!("{:.1}%", est.estimated_cpu_pct),
        );
    }

    // Top perf functions
    println!();
    println!("--- Top Perf Functions (self samples) ---");
    println!();
    println!(
        "{:<40} {:>10} {:>8} {:>16}",
        "Function", "Self", "Self%", "Category"
    );
    println!("{}", "-".repeat(78));

    let total = perf_profile.total_samples.max(1) as f64;
    for func in perf_profile.functions.iter().take(top_n) {
        let cat = classify_rust_function(&func.function_name);
        let name_display = if func.function_name.len() > 40 {
            format!("{}...", &func.function_name[..37])
        } else {
            func.function_name.clone()
        };
        println!(
            "{:<40} {:>10} {:>8} {:>16}",
            name_display,
            func.self_samples,
            format_pct(func.self_samples as f64 / total),
            format!("{}", cat),
        );
    }
}

fn print_json(
    comparisons: &[CategoryComparison],
    op_estimates: &[OperatorCpuEstimate],
    perf_profile: &PerfProfile,
    pearson: f64,
    top_n: usize,
) {
    println!("{{");
    println!("  \"pearson_correlation\": {:.4},", pearson);

    // Categories
    println!("  \"categories\": [");
    let active: Vec<&CategoryComparison> = comparisons
        .iter()
        .filter(|c| c.trace_ns > 0 || c.perf_samples > 0)
        .collect();
    for (i, c) in active.iter().enumerate() {
        let comma = if i + 1 < active.len() { "," } else { "" };
        println!(
            "    {{\"category\": \"{}\", \"trace_pct\": {:.2}, \"perf_pct\": {:.2}, \"divergence\": {:.2}}}{}",
            c.category, c.trace_pct, c.perf_pct, c.divergence, comma
        );
    }
    println!("  ],");

    // Top operators
    println!("  \"top_operators\": [");
    let ops: Vec<&OperatorCpuEstimate> = op_estimates.iter().take(top_n).collect();
    for (i, est) in ops.iter().enumerate() {
        let comma = if i + 1 < ops.len() { "," } else { "" };
        println!(
            "    {{\"operator\": {:?}, \"calls\": {}, \"trace_self_ns\": {}, \"estimated_cpu_pct\": {:.2}}}{}",
            est.operator, est.call_count, est.trace_self_ns, est.estimated_cpu_pct, comma
        );
    }
    println!("  ],");

    // Top perf functions
    let total = perf_profile.total_samples.max(1) as f64;
    println!("  \"top_perf_functions\": [");
    let funcs: Vec<_> = perf_profile.functions.iter().take(top_n).collect();
    for (i, func) in funcs.iter().enumerate() {
        let cat = classify_rust_function(&func.function_name);
        let comma = if i + 1 < funcs.len() { "," } else { "" };
        println!(
            "    {{\"function\": {:?}, \"self_samples\": {}, \"self_pct\": {:.2}, \"category\": \"{}\"}}{}",
            func.function_name,
            func.self_samples,
            func.self_samples as f64 / total * 100.0,
            cat,
            comma
        );
    }
    println!("  ]");

    println!("}}");
}
