//! Tests for the trace system.

use std::io::Read;
use std::sync::Arc;

use trace_format::*;

use super::collector::TraceCollector;
use super::convert::{trace_value, trace_value_generic, FileTable};
use crate::backend::models::metta_value::MettaValue;
use crate::backend::models::MettaValueTrait;

#[test]
fn test_bitcode_event_roundtrip() {
    let event = TraceEvent {
        seq: 42,
        thread_id: 0,
        timestamp_ns: 123456789,
        tier: TraceTier::TreeWalker,
        depth: 3,
        input: TraceValue::SExpr(vec![
            TraceValue::Atom("+".to_string()),
            TraceValue::Long(1),
            TraceValue::Long(2),
        ]),
        outputs: vec![TraceValue::Long(3)],
        expr_span: Some(TraceSpan {
            file_id: 0,
            start_row: 1,
            start_col: 0,
            end_row: 1,
            end_col: 7,
        }),
        kind: TraceEventKind::GroundedOp {
            op_name: "+".to_string(),
            args: vec![TraceValue::Long(1), TraceValue::Long(2)],
        },
    };

    // Serialize.
    let bytes = trace_format::serialize(&event);
    assert!(!bytes.is_empty());

    // Deserialize.
    let deserialized: TraceEvent =
        trace_format::deserialize(&bytes).expect("deserialization should succeed");

    assert_eq!(deserialized, event);
}

#[test]
fn test_bitcode_header_roundtrip() {
    let header = TraceHeader {
        source_file: "test.metta".to_string(),
        start_time_ns: 999,
        mettatron_version: "0.2.0".to_string(),
        cpu_count: 8,
        file_table: vec!["test.metta".to_string(), "lib.metta".to_string()],
    };

    let bytes = trace_format::serialize(&header);
    let deserialized: TraceHeader =
        trace_format::deserialize(&bytes).expect("deserialization should succeed");

    assert_eq!(deserialized, header);
}

#[test]
fn test_trace_value_display() {
    assert_eq!(TraceValue::Atom("foo".into()).to_string(), "foo");
    assert_eq!(TraceValue::Bool(true).to_string(), "True");
    assert_eq!(TraceValue::Bool(false).to_string(), "False");
    assert_eq!(TraceValue::Long(42).to_string(), "42");
    assert_eq!(TraceValue::String("hi".into()).to_string(), "\"hi\"");
    assert_eq!(TraceValue::Unit.to_string(), "()");
    assert_eq!(TraceValue::Empty.to_string(), "%void%");
    assert_eq!(
        TraceValue::SExpr(vec![
            TraceValue::Atom("+".into()),
            TraceValue::Long(1),
            TraceValue::Long(2),
        ])
        .to_string(),
        "(+ 1 2)"
    );
    assert_eq!(
        TraceValue::Error("oops".into(), Box::new(TraceValue::Unit)).to_string(),
        "(Error oops ())"
    );
    assert_eq!(
        TraceValue::Quoted(Box::new(TraceValue::Atom("x".into()))).to_string(),
        "(quote x)"
    );
}

#[test]
fn test_collector_write_and_finalize() {
    let dir = std::env::temp_dir();
    let trace_path = dir.join("test_trace.mtrace");
    let trace_path_str = trace_path.to_str().expect("valid path");

    let collector =
        TraceCollector::new(trace_path_str, "test.metta").expect("should create collector");

    // Emit a few events.
    let input = MettaValue::SExpr(vec![
        MettaValue::Atom("+"),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let output = MettaValue::Long(3);

    for i in 0..5 {
        collector.emit(
            TraceTier::TreeWalker,
            i as u32,
            &input,
            &[output],
            None,
            TraceEventKind::GroundedOp {
                op_name: "+".to_string(),
                args: vec![TraceValue::Long(1), TraceValue::Long(2)],
            },
        );
    }

    let count = collector.finalize().expect("finalize should succeed");
    assert_eq!(count, 5);

    // Verify the file starts with magic bytes.
    let mut file = std::fs::File::open(&trace_path).expect("should open trace file");
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).expect("should read magic");
    assert_eq!(magic, TRACE_MAGIC);

    // Clean up.
    let _ = std::fs::remove_file(&trace_path);
}

#[test]
fn test_trace_value_conversion_deep_nesting() {
    // Build a deeply nested S-expression: (a (b (c (d ... ))))
    let depth = 100;
    let mut val = MettaValue::Atom("leaf");
    for i in (0..depth).rev() {
        let name_str = if i % 2 == 0 { "even" } else { "odd" };
        val = MettaValue::SExpr(vec![MettaValue::Atom(name_str), val]);
    }

    // Should not stack overflow.
    let tv = trace_value(&val);

    // Verify structure.
    fn check_depth(tv: &TraceValue, expected_depth: usize) -> bool {
        if expected_depth == 0 {
            return matches!(tv, TraceValue::Atom(s) if s == "leaf");
        }
        match tv {
            TraceValue::SExpr(items) if items.len() == 2 => {
                check_depth(&items[1], expected_depth - 1)
            }
            _ => false,
        }
    }
    assert!(check_depth(&tv, depth));
}

#[test]
fn test_file_table() {
    let mut ft = FileTable::new();
    assert_eq!(ft.intern("a.metta"), 0);
    assert_eq!(ft.intern("b.metta"), 1);
    assert_eq!(ft.intern("a.metta"), 0); // Dedup.
    assert_eq!(ft.intern("c.metta"), 2);
    let paths = ft.into_paths();
    assert_eq!(paths, vec!["a.metta", "b.metta", "c.metta"]);
}

#[test]
fn test_bitcode_recursive_tracevalue_roundtrip() {
    // Test that recursive TraceValue types serialize/deserialize correctly.
    let nested = TraceValue::SExpr(vec![
        TraceValue::Atom("f".into()),
        TraceValue::Error(
            "err".into(),
            Box::new(TraceValue::Type(Box::new(TraceValue::Quoted(Box::new(
                TraceValue::SExpr(vec![TraceValue::Long(1), TraceValue::Float(2.0)]),
            ))))),
        ),
    ]);

    let bytes = trace_format::serialize(&nested);
    let deserialized: TraceValue =
        trace_format::deserialize(&bytes).expect("recursive deserialize should succeed");
    assert_eq!(deserialized, nested);
}

#[test]
fn test_trace_value_generic_matches_concrete() {
    // Verify that trace_value_generic (trait-based) produces the same output
    // as trace_value (concrete MettaValueInner-based) for all value types.
    use crate::backend::models::{global_factory, MettaValueFactory};
    let factory = global_factory();

    let test_cases: Vec<MettaValue> = vec![
        factory.atom("hello"),
        factory.bool(true),
        factory.bool(false),
        factory.long(42),
        factory.float(3.14),
        factory.string("world"),
        factory.unit(),
        factory.sexpr(vec![factory.atom("+"), factory.long(1), factory.long(2)]),
        factory.error("oops", factory.unit()),
    ];

    for val in &test_cases {
        let concrete = trace_value(val);
        let generic = trace_value_generic(val);
        assert_eq!(
            concrete, generic,
            "trace_value and trace_value_generic disagree for {:?}",
            val
        );
    }
}

#[test]
fn test_end_to_end_trace_file() {
    // Compile and evaluate a small MeTTa program with tracing enabled.
    // Verify that the trace file contains expected events.
    use crate::backend::eval::trampoline::{eval_trampoline_with_trace, new_env};

    let dir = std::env::temp_dir();
    let trace_path = dir.join("e2e_trace.mtrace");
    let trace_path_str = trace_path.to_str().expect("valid path");

    // Compile a simple program
    let state = crate::compile("!(+ 1 2)").expect("compile should succeed");
    let mut env = new_env();

    // Create trace collector
    let collector =
        TraceCollector::new(trace_path_str, "test_e2e.metta").expect("should create collector");

    // Evaluate with trace
    let source_exprs: Vec<MettaValue> = state.source().iter().copied().collect();
    for expr in source_exprs {
        let (results, new_env) = eval_trampoline_with_trace(expr, env, &state, &collector);
        env = new_env;
        // Should produce [3]
        assert_eq!(results.len(), 1, "expected 1 result");
        assert_eq!(results[0].as_long(), Some(3), "expected 3");
    }

    // Finalize
    let count = collector.finalize().expect("finalize should succeed");
    assert!(count > 0, "expected at least one trace event, got {count}");

    // Verify the file starts with magic bytes
    let mut file = std::fs::File::open(&trace_path).expect("should open trace file");
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic).expect("should read magic");
    assert_eq!(magic, TRACE_MAGIC, "trace file should start with magic bytes");

    // Clean up
    let _ = std::fs::remove_file(&trace_path);
}

#[test]
fn test_thread_local_trace_sink() {
    // Verify the thread-local trace collector set/clear/access pattern.
    use super::thread_local_sink::{
        set_thread_trace_collector, clear_thread_trace_collector, with_thread_trace_collector,
    };

    let dir = std::env::temp_dir();
    let trace_path = dir.join("tls_trace.mtrace");
    let trace_path_str = trace_path.to_str().expect("valid path");

    // Initially, no collector should be set
    let result = with_thread_trace_collector(|_tc| true);
    assert!(result.is_none(), "no collector should be set initially");

    // Set a collector
    let collector =
        TraceCollector::new(trace_path_str, "tls_test.metta").expect("should create collector");
    set_thread_trace_collector(&collector);

    // Now the closure should fire
    let result = with_thread_trace_collector(|tc| {
        tc.emit_converted(
            TraceTier::TreeWalker, 0,
            TraceValue::Unit, vec![], None,
            TraceEventKind::EvalStart,
        );
        true
    });
    assert_eq!(result, Some(true), "collector should be accessible");

    // Clear
    clear_thread_trace_collector();
    let result = with_thread_trace_collector(|_tc| true);
    assert!(result.is_none(), "collector should be cleared");

    // Finalize
    let count = collector.finalize().expect("finalize should succeed");
    assert_eq!(count, 1, "expected 1 event from thread-local test");

    // Clean up
    let _ = std::fs::remove_file(&trace_path);
}
