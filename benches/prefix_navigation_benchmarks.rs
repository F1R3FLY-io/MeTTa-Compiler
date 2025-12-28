#![feature(test)]
#![allow(clippy::missing_docs_in_private_items)]
extern crate test;

use mettatron::backend::{Environment, MettaValue};

/// Generate test environment with N type assertions and M rules
#[allow(dead_code)]
fn generate_test_env(num_types: usize, num_rules: usize) -> Environment {
    let mut env = Environment::new();

    // Add type assertions (: name type)
    for i in 0..num_types {
        let name = format!("var{}", i);
        let typ = MettaValue::Atom(format!("Type{}", i % 10));
        env.add_type(name, typ);
    }

    // Add rules for realistic space structure
    for i in 0..num_rules {
        let lhs = MettaValue::SExpr(vec![
            MettaValue::Atom(format!("func{}", i % 50)),
            MettaValue::Atom("$x".to_string()),
        ]);
        let rhs = MettaValue::Atom(format!("result{}", i));
        env.add_rule(mettatron::backend::Rule::new(lhs, rhs));
    }

    env
}

// ============================================================================
// BASELINE: get_type() - Current O(n) Implementation
// ============================================================================

#[bench]
fn bench_get_type_baseline_10_types(b: &mut Bencher) {
    let env = generate_test_env(10, 0);
    b.iter(|| env.get_type("var5"));
}

#[bench]
fn bench_get_type_baseline_100_types(b: &mut Bencher) {
    let env = generate_test_env(100, 0);
    b.iter(|| env.get_type("var50"));
}

#[bench]
fn bench_get_type_baseline_1000_types(b: &mut Bencher) {
    let env = generate_test_env(1000, 0);
    b.iter(|| env.get_type("var500"));
}

#[bench]
fn bench_get_type_baseline_10000_types(b: &mut Bencher) {
    let env = generate_test_env(10000, 0);
    b.iter(|| env.get_type("var5000"));
}

// ============================================================================
// BASELINE: get_type() with Mixed Workload (types + rules)
// ============================================================================

#[bench]
fn bench_get_type_baseline_mixed_small(b: &mut Bencher) {
    let env = generate_test_env(50, 50);
    b.iter(|| env.get_type("var25"));
}

#[bench]
fn bench_get_type_baseline_mixed_medium(b: &mut Bencher) {
    let env = generate_test_env(500, 500);
    b.iter(|| env.get_type("var250"));
}

#[bench]
fn bench_get_type_baseline_mixed_large(b: &mut Bencher) {
    let env = generate_test_env(5000, 5000);
    b.iter(|| env.get_type("var2500"));
}

// ============================================================================
// BASELINE: match_space() - Pattern Matching
// ============================================================================

#[bench]
fn bench_match_space_baseline_10_facts(b: &mut Bencher) {
    let mut env = Environment::new();

    // Add facts
    for i in 0..10 {
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("data".to_string()),
            MettaValue::Long(i),
        ]);
        env.add_to_space(&fact);
    }

    let pattern = MettaValue::SExpr(vec![
        MettaValue::Atom("data".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);

    let template = MettaValue::Atom("$x".to_string());

    b.iter(|| env.match_space(&pattern, &template));
}

#[bench]
fn bench_match_space_baseline_100_facts(b: &mut Bencher) {
    let mut env = Environment::new();

    for i in 0..100 {
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("data".to_string()),
            MettaValue::Long(i),
        ]);
        env.add_to_space(&fact);
    }

    let pattern = MettaValue::SExpr(vec![
        MettaValue::Atom("data".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);

    let template = MettaValue::Atom("$x".to_string());

    b.iter(|| env.match_space(&pattern, &template));
}

#[bench]
fn bench_match_space_baseline_1000_facts(b: &mut Bencher) {
    let mut env = Environment::new();

    for i in 0..1000 {
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom("data".to_string()),
            MettaValue::Long(i),
        ]);
        env.add_to_space(&fact);
    }

    let pattern = MettaValue::SExpr(vec![
        MettaValue::Atom("data".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);

    let template = MettaValue::Atom("$x".to_string());

    b.iter(|| env.match_space(&pattern, &template));
}

// ============================================================================
// BASELINE: Sparse Queries (worst case for linear search)
// ============================================================================

#[bench]
fn bench_get_type_sparse_1_in_100(b: &mut Bencher) {
    let env = generate_test_env(100, 0);
    b.iter(|| {
        // Query for type that exists near end
        env.get_type("var95")
    });
}

#[bench]
fn bench_get_type_sparse_1_in_1000(b: &mut Bencher) {
    let env = generate_test_env(1000, 0);
    b.iter(|| {
        // Query for type that exists near end
        env.get_type("var950")
    });
}

#[bench]
fn bench_get_type_sparse_1_in_10000(b: &mut Bencher) {
    let env = generate_test_env(10000, 0);
    b.iter(|| {
        // Query for type that exists near end
        env.get_type("var9500")
    });
}

// ============================================================================
// BASELINE: has_fact() - Atom Search
// ============================================================================

#[bench]
fn bench_has_fact_baseline_10_facts(b: &mut Bencher) {
    let mut env = Environment::new();

    for i in 0..10 {
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom(format!("atom{}", i)),
            MettaValue::Long(i),
        ]);
        env.add_to_space(&fact);
    }

    b.iter(|| env.has_fact("atom5"));
}

#[bench]
fn bench_has_fact_baseline_100_facts(b: &mut Bencher) {
    let mut env = Environment::new();

    for i in 0..100 {
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom(format!("atom{}", i)),
            MettaValue::Long(i),
        ]);
        env.add_to_space(&fact);
    }

    b.iter(|| env.has_fact("atom50"));
}

#[bench]
fn bench_has_fact_baseline_1000_facts(b: &mut Bencher) {
    let mut env = Environment::new();

    for i in 0..1000 {
        let fact = MettaValue::SExpr(vec![
            MettaValue::Atom(format!("atom{}", i)),
            MettaValue::Long(i),
        ]);
        env.add_to_space(&fact);
    }

    b.iter(|| env.has_fact("atom500"));
}

// ============================================================================
// BASELINE: Iteration Overhead
// ============================================================================

#[bench]
fn bench_iter_rules_10_rules(b: &mut Bencher) {
    let env = generate_test_env(0, 10);
    b.iter(|| env.iter_rules().count());
}

#[bench]
fn bench_iter_rules_100_rules(b: &mut Bencher) {
    let env = generate_test_env(0, 100);
    b.iter(|| env.iter_rules().count());
}

#[bench]
fn bench_iter_rules_1000_rules(b: &mut Bencher) {
    let env = generate_test_env(0, 1000);
    b.iter(|| env.iter_rules().count());
}

// Required by harness = false in Cargo.toml for #[bench] attribute tests
fn main() {}
