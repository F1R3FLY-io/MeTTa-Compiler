//! JIT/bytecode bailout event summary.

use std::collections::HashMap;

use crate::reader::TraceReader;
use trace_format::TraceEventKind;

pub fn run(file: &str) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    let mut jit_bailouts = Vec::new();
    let mut bytecode_halts = Vec::new();
    let mut bailout_reasons: HashMap<String, u64> = HashMap::new();

    for event in reader.events() {
        match &event.kind {
            TraceEventKind::JitBailout { bailout_ip, reason, fallback_tier } => {
                jit_bailouts.push((event.seq, *bailout_ip, reason.clone(), fallback_tier.clone()));
                *bailout_reasons.entry(reason.clone()).or_default() += 1;
            }
            TraceEventKind::BytecodeHalt { ip, reason } => {
                bytecode_halts.push((event.seq, *ip, reason.clone()));
                *bailout_reasons.entry(format!("BytecodeHalt: {}", reason)).or_default() += 1;
            }
            _ => {}
        }
    }

    println!("=== Bailout Summary ===");
    println!();

    if jit_bailouts.is_empty() && bytecode_halts.is_empty() {
        println!("No bailouts recorded.");
        return Ok(());
    }

    // Reason summary
    println!("--- Bailout Reasons ---");
    let mut reasons_vec: Vec<_> = bailout_reasons.into_iter().collect();
    reasons_vec.sort_by(|a, b| b.1.cmp(&a.1));
    for (reason, count) in &reasons_vec {
        println!("  {:>6}x  {}", count, reason);
    }

    // JIT bailouts detail
    if !jit_bailouts.is_empty() {
        println!();
        println!("--- JIT Bailouts ({}) ---", jit_bailouts.len());
        for (seq, ip, reason, fallback) in jit_bailouts.iter().take(50) {
            println!("  [#{}] ip={}, reason: {}, fallback: {}", seq, ip, reason, fallback);
        }
        if jit_bailouts.len() > 50 {
            println!("  ... and {} more", jit_bailouts.len() - 50);
        }
    }

    // Bytecode halts detail
    if !bytecode_halts.is_empty() {
        println!();
        println!("--- Bytecode Halts ({}) ---", bytecode_halts.len());
        for (seq, ip, reason) in bytecode_halts.iter().take(50) {
            println!("  [#{}] ip={}, reason: \"{}\"", seq, ip, reason);
        }
        if bytecode_halts.len() > 50 {
            println!("  ... and {} more", bytecode_halts.len() - 50);
        }
    }

    println!();
    println!("Total: {} JIT bailouts, {} bytecode halts", jit_bailouts.len(), bytecode_halts.len());
    Ok(())
}
