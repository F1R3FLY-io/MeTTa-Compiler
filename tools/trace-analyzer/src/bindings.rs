//! Per-BoundValue binding-flow analysis (v5 events).
//!
//! Reads `ContinuationEnter` / `ContinuationEmit` / `ContinuationExitNoResume`
//! / `BindingsDropped` events emitted by the trampoline's dispatcher-level
//! instrumentation (eval-trace feature). Surfaces where per-branch bindings
//! are preserved, composed, or silently dropped between continuation
//! handlers — the primary use case is pinpointing the `.map(|(v, _)| v)`
//! drop sites that silently collapse nondeterministic alternatives'
//! bindings.
//!
//! ## Output modes
//!
//! - Default: per-`flow_id` timeline with ENTER / EMIT / DROPPED lines.
//! - `--drops-only`: suppresses everything except `BindingsDropped` events
//!   plus a summary table grouped by `(cont_kind, site)` with top dropped
//!   variable names.
//!
//! Filtering is analyzer-side only; the emitter has no env-var switches.

use crate::dump::format_trace_value;
use crate::reader::TraceReader;
use std::collections::{BTreeMap, HashMap};
use trace_format::{BoundValueSnapshot, TraceEventKind};

pub fn run(
    file: &str,
    drops_only: bool,
    var_filter: Option<Vec<String>>,
    cont_filter: Option<Vec<String>>,
    limit: Option<usize>,
) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    println!("Trace file: {}", reader.header.source_file);
    println!("MeTTaTron version: {}", reader.header.mettatron_version);
    println!();

    // Accumulate events per flow_id so Enter and Emit/Dropped/Exit for
    // the same boundary render together.
    struct Flow {
        cont_kind: String,
        cont_depth: u32,
        inputs: Vec<BoundValueSnapshot>,
        tracked_vars: Vec<String>,
        emits: Vec<(String, Vec<BoundValueSnapshot>)>,
        drops: Vec<(String, Vec<String>, Vec<(String, trace_format::TraceValue)>)>,
        exit: Option<String>,
    }

    let mut flows: BTreeMap<u64, Flow> = BTreeMap::new();
    // Order flows by first-seen for deterministic output.
    let mut flow_order: Vec<u64> = Vec::new();

    // Per-(cont_kind, site) drop counter for the summary table.
    let mut drop_summary: HashMap<(String, String), (u64, HashMap<String, u64>)> =
        HashMap::new();

    let cont_whitelist = cont_filter.map(|v| v.into_iter().collect::<std::collections::HashSet<_>>());
    let var_whitelist = var_filter.map(|v| v.into_iter().collect::<std::collections::HashSet<_>>());

    for event in reader.events() {
        match &event.kind {
            TraceEventKind::ContinuationEnter {
                cont_kind,
                flow_id,
                cont_depth,
                inputs,
                tracked_vars,
            } => {
                if let Some(ref cw) = cont_whitelist {
                    if !cw.contains(cont_kind) {
                        continue;
                    }
                }
                if let Some(ref vw) = var_whitelist {
                    let touches = inputs.iter().any(|bv| {
                        bv.bindings.iter().any(|(k, _)| vw.contains(k))
                    }) || tracked_vars.iter().any(|k| vw.contains(k));
                    if !touches {
                        continue;
                    }
                }
                if !flows.contains_key(flow_id) {
                    flow_order.push(*flow_id);
                }
                flows.insert(
                    *flow_id,
                    Flow {
                        cont_kind: cont_kind.clone(),
                        cont_depth: *cont_depth,
                        inputs: inputs.clone(),
                        tracked_vars: tracked_vars.clone(),
                        emits: Vec::new(),
                        drops: Vec::new(),
                        exit: None,
                    },
                );
            }
            TraceEventKind::ContinuationEmit {
                flow_id, site, outputs, ..
            } => {
                if let Some(f) = flows.get_mut(flow_id) {
                    f.emits.push((site.clone(), outputs.clone()));
                }
            }
            TraceEventKind::ContinuationExitNoResume { flow_id, exit_kind, .. } => {
                if let Some(f) = flows.get_mut(flow_id) {
                    f.exit = Some(exit_kind.clone());
                }
            }
            TraceEventKind::BindingsDropped {
                cont_kind,
                flow_id,
                site,
                dropped_keys,
                sample,
            } => {
                if let Some(ref cw) = cont_whitelist {
                    if !cw.contains(cont_kind) {
                        continue;
                    }
                }
                if let Some(ref vw) = var_whitelist {
                    let touches = dropped_keys.iter().any(|k| vw.contains(k));
                    if !touches {
                        continue;
                    }
                }
                if let Some(f) = flows.get_mut(flow_id) {
                    f.drops
                        .push((site.clone(), dropped_keys.clone(), sample.clone()));
                }
                let entry = drop_summary
                    .entry((cont_kind.clone(), site.clone()))
                    .or_insert_with(|| (0, HashMap::new()));
                entry.0 += 1;
                for k in dropped_keys {
                    *entry.1.entry(k.clone()).or_insert(0) += 1;
                }
            }
            _ => {}
        }
    }

    if drops_only {
        let mut drop_events = 0usize;
        for id in &flow_order {
            let f = match flows.get(id) {
                Some(f) => f,
                None => continue,
            };
            if f.drops.is_empty() {
                continue;
            }
            if let Some(l) = limit {
                if drop_events >= l {
                    println!("... (truncated at {l} flows)");
                    break;
                }
            }
            let tv = if f.tracked_vars.is_empty() {
                String::new()
            } else {
                format!(" tracked={:?}", f.tracked_vars)
            };
            println!(
                "flow#{} {} depth={}{}",
                id, f.cont_kind, f.cont_depth, tv,
            );
            println!("  ENTER inputs={}", format_bvs(&f.inputs));
            for (site, keys, sample) in &f.drops {
                let sample_str: Vec<String> = sample
                    .iter()
                    .map(|(k, v)| format!("{}→{}", k, format_trace_value(v)))
                    .collect();
                println!(
                    "  ⚠ DROPPED site={} keys={:?} sample=[{}]",
                    site,
                    keys,
                    sample_str.join(", ")
                );
            }
            drop_events += 1;
        }

        println!();
        print_summary(&drop_summary);
        return Ok(());
    }

    let mut shown = 0usize;
    for id in &flow_order {
        let f = match flows.get(id) {
            Some(f) => f,
            None => continue,
        };
        if let Some(l) = limit {
            if shown >= l {
                println!("... (truncated at {l} flows)");
                break;
            }
        }
        println!(
            "flow#{} {} depth={}",
            id, f.cont_kind, f.cont_depth
        );
        println!("  ENTER inputs={}", format_bvs(&f.inputs));
        for (site, outputs) in &f.emits {
            println!("  EMIT  site={} outputs={}", site, format_bvs(outputs));
        }
        for (site, keys, sample) in &f.drops {
            let sample_str: Vec<String> = sample
                .iter()
                .map(|(k, v)| format!("{}→{}", k, format_trace_value(v)))
                .collect();
            println!(
                "  ⚠ DROPPED site={} keys={:?} sample=[{}]",
                site,
                keys,
                sample_str.join(", ")
            );
        }
        if let Some(ref e) = f.exit {
            println!("  EXIT  kind={}", e);
        }
        shown += 1;
    }

    println!();
    print_summary(&drop_summary);
    Ok(())
}

fn format_bvs(bvs: &[BoundValueSnapshot]) -> String {
    if bvs.is_empty() {
        return "[]".to_string();
    }
    let parts: Vec<String> = bvs
        .iter()
        .map(|bv| {
            let b: Vec<String> = bv
                .bindings
                .iter()
                .map(|(k, v)| format!("{}={}", k, format_trace_value(v)))
                .collect();
            if b.is_empty() {
                format!("({} {{}})", format_trace_value(&bv.value))
            } else {
                format!("({} {{{}}})", format_trace_value(&bv.value), b.join(","))
            }
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

fn print_summary(
    drop_summary: &HashMap<(String, String), (u64, HashMap<String, u64>)>,
) {
    if drop_summary.is_empty() {
        println!("Drop summary: no BindingsDropped events.");
        return;
    }
    println!("Drop summary by (cont_kind, site):");
    let mut rows: Vec<_> = drop_summary.iter().collect();
    rows.sort_by_key(|(_, (count, _))| std::cmp::Reverse(*count));
    for ((cont_kind, site), (count, var_counts)) in rows.iter().take(30) {
        let mut vars: Vec<(&String, &u64)> = var_counts.iter().collect();
        vars.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        let top_vars: Vec<String> = vars
            .iter()
            .take(5)
            .map(|(k, c)| format!("{}({})", k, c))
            .collect();
        println!(
            "  {}/{}   {} drop(s)   vars: {}",
            cont_kind,
            site,
            count,
            top_vars.join(", ")
        );
    }
}
