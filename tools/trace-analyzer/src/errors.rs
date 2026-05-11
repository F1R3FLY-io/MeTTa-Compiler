//! Error/exception event listing with context chain.

use crate::reader::TraceReader;
use trace_format::{TraceEventKind, TraceValue};

pub fn run(file: &str) -> Result<(), String> {
    let reader = TraceReader::open(file)?;

    println!("=== Error Events ===");
    println!();

    let mut error_count = 0u64;
    for event in reader.events() {
        match &event.kind {
            TraceEventKind::ErrorCreated { message, details } => {
                error_count += 1;
                println!(
                    "[#{} T{} D{} {:?}] ERROR CREATED",
                    event.seq, event.thread_id, event.depth, event.tier,
                );
                println!("  message: \"{}\"", message);
                println!("  details: {}", format_value(details));
                println!();
            }
            TraceEventKind::GroundedOpError {
                op_name,
                error_kind,
                message,
                args,
            } => {
                error_count += 1;
                println!(
                    "[#{} T{} D{} {:?}] GROUNDED OP ERROR",
                    event.seq, event.thread_id, event.depth, event.tier,
                );
                println!("  op: \"{}\"", op_name);
                println!("  kind: \"{}\"", error_kind);
                println!("  message: \"{}\"", message);
                let args_str: Vec<String> = args.iter().map(format_value).collect();
                println!("  args: [{}]", args_str.join(", "));
                println!();
            }
            TraceEventKind::ErrorCaught {
                error,
                handler,
                default_used,
            } => {
                error_count += 1;
                println!(
                    "[#{} T{} D{} {:?}] ERROR CAUGHT",
                    event.seq, event.thread_id, event.depth, event.tier,
                );
                println!("  error: {}", format_value(error));
                println!("  handler: \"{}\"", handler);
                if let Some(def) = default_used {
                    println!("  default: {}", format_value(def));
                }
                println!();
            }
            TraceEventKind::ErrorPropagated { error, context } => {
                error_count += 1;
                println!(
                    "[#{} T{} D{} {:?}] ERROR PROPAGATED",
                    event.seq, event.thread_id, event.depth, event.tier,
                );
                println!("  error: {}", format_value(error));
                println!("  context: \"{}\"", context);
                println!();
            }
            _ => {}
        }
    }

    println!("Total error events: {error_count}");
    Ok(())
}

fn format_value(v: &TraceValue) -> String {
    match v {
        TraceValue::Atom(s) => s.clone(),
        TraceValue::Bool(b) => {
            if *b {
                "True".to_string()
            } else {
                "False".to_string()
            }
        }
        TraceValue::Long(n) => n.to_string(),
        TraceValue::Float(f) => f.to_string(),
        TraceValue::String(s) => format!("\"{}\"", s),
        TraceValue::SExpr(items) => {
            let parts: Vec<String> = items.iter().map(format_value).collect();
            format!("({})", parts.join(" "))
        }
        TraceValue::Unit => "()".to_string(),
        TraceValue::Error(msg, details) => format!("(Error {} {})", msg, format_value(details)),
        TraceValue::Type(inner) => format!("Type({})", format_value(inner)),
        TraceValue::Empty => "Empty".to_string(),
        TraceValue::Quoted(inner) => format!("(quote {})", format_value(inner)),
    }
}
