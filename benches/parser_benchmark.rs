//! Comparative benchmark: Tree-Sitter parser vs Custom hand-written parser
//!
//! Benchmarks parsing at multiple scales:
//! - Small: fib.metta (~95 B)
//! - Medium: verify_demo0.metta (~3 KB)
//! - Large: mmverify-utils.metta (~21 KB)
//! - Synthetic: generated nested S-expressions (~100 KB)
//! - String-heavy: generated input with many escape sequences (~10 KB)

use divan::black_box;
use mettatron::parser::{IrEmitter, MettaParser, ValueEmitter};
use mettatron::tree_sitter_parser::TreeSitterMettaParser;
use mettatron::global_factory;

fn main() {
    divan::main();
}

// ============================================================================
// Static input data
// ============================================================================

const SMALL_SRC: &str = include_str!("metta_samples/fib.metta");
const MEDIUM_SRC: &str = include_str!("../examples/mmverify/demo0/verify_demo0.metta");
const LARGE_SRC: &str = include_str!("../examples/mmverify/mmverify-utils.metta");

// ============================================================================
// Synthetic input generators
// ============================================================================

fn generate_nested_sexprs(depth: usize, breadth: usize) -> String {
    let mut buf = String::with_capacity(100_000);
    generate_nested_recursive(&mut buf, depth, breadth);
    buf
}

fn generate_nested_recursive(buf: &mut String, depth: usize, breadth: usize) {
    if depth == 0 {
        buf.push_str("atom");
        return;
    }
    buf.push('(');
    buf.push_str("op");
    for _ in 0..breadth {
        buf.push(' ');
        generate_nested_recursive(buf, depth - 1, breadth);
    }
    buf.push(')');
}

fn generate_string_heavy(count: usize) -> String {
    let mut buf = String::with_capacity(count * 50);
    for i in 0..count {
        buf.push_str(&format!(
            "(print \"string-{} with escapes: \\n\\t\\x41\\u{{1F4A1}}\")\n",
            i
        ));
    }
    buf
}

fn generate_mixed_atoms(count: usize) -> String {
    let mut buf = String::with_capacity(count * 40);
    for i in 0..count {
        buf.push_str(&format!(
            "(= (fn-{} $x) (+ $x {}))\n!(fn-{} {})\n",
            i, i, i, i * 2
        ));
    }
    buf
}

// ============================================================================
// Small input benchmarks
// ============================================================================

#[divan::bench]
fn small_tree_sitter_parse(bencher: divan::Bencher) {
    bencher.bench_local(|| {
        let mut parser = TreeSitterMettaParser::new().expect("parser init");
        black_box(parser.parse(SMALL_SRC).expect("parse"));
    });
}

#[divan::bench]
fn small_custom_parse_to_ir(bencher: divan::Bencher) {
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(SMALL_SRC);
        let mut emitter = IrEmitter::new();
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

#[divan::bench]
fn small_custom_parse_to_value(bencher: divan::Bencher) {
    let factory = global_factory();
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(SMALL_SRC);
        let mut emitter = ValueEmitter::new(&factory);
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

// ============================================================================
// Medium input benchmarks
// ============================================================================

#[divan::bench]
fn medium_tree_sitter_parse(bencher: divan::Bencher) {
    bencher.bench_local(|| {
        let mut parser = TreeSitterMettaParser::new().expect("parser init");
        black_box(parser.parse(MEDIUM_SRC).expect("parse"));
    });
}

#[divan::bench]
fn medium_custom_parse_to_ir(bencher: divan::Bencher) {
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(MEDIUM_SRC);
        let mut emitter = IrEmitter::new();
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

#[divan::bench]
fn medium_custom_parse_to_value(bencher: divan::Bencher) {
    let factory = global_factory();
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(MEDIUM_SRC);
        let mut emitter = ValueEmitter::new(&factory);
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

// ============================================================================
// Large input benchmarks
// ============================================================================

#[divan::bench]
fn large_tree_sitter_parse(bencher: divan::Bencher) {
    bencher.bench_local(|| {
        let mut parser = TreeSitterMettaParser::new().expect("parser init");
        black_box(parser.parse(LARGE_SRC).expect("parse"));
    });
}

#[divan::bench]
fn large_custom_parse_to_ir(bencher: divan::Bencher) {
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(LARGE_SRC);
        let mut emitter = IrEmitter::new();
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

#[divan::bench]
fn large_custom_parse_to_value(bencher: divan::Bencher) {
    let factory = global_factory();
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(LARGE_SRC);
        let mut emitter = ValueEmitter::new(&factory);
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

// ============================================================================
// Synthetic input benchmarks
// ============================================================================

#[divan::bench]
fn synthetic_nested_tree_sitter(bencher: divan::Bencher) {
    let input = generate_nested_sexprs(6, 4); // ~100KB nested S-exprs
    bencher.bench_local(|| {
        let mut parser = TreeSitterMettaParser::new().expect("parser init");
        black_box(parser.parse(&input).expect("parse"));
    });
}

#[divan::bench]
fn synthetic_nested_custom_ir(bencher: divan::Bencher) {
    let input = generate_nested_sexprs(6, 4);
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(&input);
        let mut emitter = IrEmitter::new();
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

#[divan::bench]
fn synthetic_nested_custom_value(bencher: divan::Bencher) {
    let input = generate_nested_sexprs(6, 4);
    let factory = global_factory();
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(&input);
        let mut emitter = ValueEmitter::new(&factory);
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

// ============================================================================
// String-heavy input benchmarks
// ============================================================================

#[divan::bench]
fn strings_tree_sitter(bencher: divan::Bencher) {
    let input = generate_string_heavy(200);
    bencher.bench_local(|| {
        let mut parser = TreeSitterMettaParser::new().expect("parser init");
        black_box(parser.parse(&input).expect("parse"));
    });
}

#[divan::bench]
fn strings_custom_ir(bencher: divan::Bencher) {
    let input = generate_string_heavy(200);
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(&input);
        let mut emitter = IrEmitter::new();
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

#[divan::bench]
fn strings_custom_value(bencher: divan::Bencher) {
    let input = generate_string_heavy(200);
    let factory = global_factory();
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(&input);
        let mut emitter = ValueEmitter::new(&factory);
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

// ============================================================================
// Mixed atoms input benchmarks
// ============================================================================

#[divan::bench]
fn mixed_tree_sitter(bencher: divan::Bencher) {
    let input = generate_mixed_atoms(500);
    bencher.bench_local(|| {
        let mut parser = TreeSitterMettaParser::new().expect("parser init");
        black_box(parser.parse(&input).expect("parse"));
    });
}

#[divan::bench]
fn mixed_custom_ir(bencher: divan::Bencher) {
    let input = generate_mixed_atoms(500);
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(&input);
        let mut emitter = IrEmitter::new();
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}

#[divan::bench]
fn mixed_custom_value(bencher: divan::Bencher) {
    let input = generate_mixed_atoms(500);
    let factory = global_factory();
    bencher.bench_local(|| {
        let mut parser = MettaParser::new(&input);
        let mut emitter = ValueEmitter::new(&factory);
        black_box(parser.parse_all(&mut emitter).expect("parse"));
    });
}
