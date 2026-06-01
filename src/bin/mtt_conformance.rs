//! `mtt-conformance` — conformance test runner for MeTTaTron.
//!
//! Walks a `conformance/` directory (typically
//! `mettatron-specification/conformance/`), reads each `.metta` file plus its
//! sibling `.expected.yaml`, runs MeTTaTron on every applicable tier, and
//! reports per-tier PASS/FAIL/DEMOTED. Per spec §K and Workstream H phases.
//!
//! Usage:
//!   mtt-conformance --conformance-dir <PATH>
//!   mtt-conformance --conformance-dir <PATH> --tier T0,T1,T2,T3
//!   mtt-conformance --conformance-dir <PATH> --module M11-bisimilarity-he
//!   mtt-conformance --conformance-dir <PATH> --fixture 008-if-2arg
//!
//! Exit codes:
//!   0  — all fixtures pass on all declared-normative tiers
//!   3  — at least one fixture/tier diverged from the expected output
//!   5  — fixture directory missing or unreadable
//!
//! The expected.yaml schema is the spec's existing fixture schema. This
//! runner does NOT parse YAML structure exhaustively — it extracts the
//! `results:` field and compares with the canonical multiset normalization
//! (sort lexicographically per spec §20.1.2).

use std::env;
use std::fs;
use std::path::Path;
use std::process;

#[path = "conformance_common.rs"]
mod conformance_common;

use conformance_common::{
    atom_kind_prefix, canonicalize, collect_fixtures_recursive, discover_fixtures, parse_base_args,
    read_status, split_yaml_list_inner, strip_yaml_quotes, BaseOptions, FixtureOutcome,
};

use mettatron::backend::eval::tier_forced::{
    eval_with_tier, FallbackPolicy, TierEvalOutcome, TierSelection,
};
use mettatron::backend::eval::trampoline::new_env;
use mettatron::{compile, MettaValue};

fn print_usage() {
    eprintln!("mtt-conformance — conformance test runner for MeTTaTron");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    mtt-conformance --conformance-dir <PATH> [--module <NAME>] [--fixture <NAME>]");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    --conformance-dir <PATH>  Root of the conformance/ directory");
    eprintln!(
        "    --module <NAME>           Only run fixtures in this subdir (e.g. M11-bisimilarity-he)"
    );
    eprintln!("    --fixture <NAME>          Only run fixtures whose path-tail contains <NAME>");
    eprintln!("    --tier <T1,T2,...>        Restrict to specific tiers (default: all)");
    eprintln!("    --strict                  Exit code 3 on any divergence (default: lenient)");
    eprintln!("    --help                    Show this help");
    eprintln!();
    eprintln!("EXIT CODES:");
    eprintln!("    0  — all fixtures pass");
    eprintln!("    3  — divergence detected (with --strict)");
    eprintln!("    5  — directory missing");
}

/// Find the outer `[` ... `]` bracket pair in a YAML line, skipping
/// brackets that appear inside `"..."` or `'...'` quoted strings. Returns
/// the byte indices of the matching pair, or None if not found.
fn find_outer_bracket(body: &str) -> Option<(usize, usize)> {
    let mut lo: Option<usize> = None;
    let mut in_double_quote = false;
    let mut in_single_quote = false;
    let mut escape = false;
    for (i, ch) in body.char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_double_quote => {
                escape = true;
            }
            '"' if !in_single_quote => {
                in_double_quote = !in_double_quote;
            }
            '\'' if !in_double_quote => {
                in_single_quote = !in_single_quote;
            }
            '[' if !in_double_quote && !in_single_quote => {
                if lo.is_none() {
                    lo = Some(i);
                }
            }
            ']' if !in_double_quote && !in_single_quote => {
                if let Some(l) = lo {
                    return Some((l, i));
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse the expected `results:` block from an `.expected.yaml` fixture.
///
/// The schema is a YAML list under `results:`:
/// ```yaml
/// results:
///   - atoms: ["True"]
///   - atoms: ["False"]
/// ```
fn parse_expected_results(yaml_path: &Path) -> Option<Vec<Vec<String>>> {
    let content = fs::read_to_string(yaml_path).ok()?;
    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut in_results = false;
    let mut explicit_empty = false;
    let mut current_group: Option<Vec<String>> = None;
    for raw_line in content.lines() {
        let trimmed = raw_line.trim_end();
        let indent = trimmed.len() - trimmed.trim_start().len();
        let body = trimmed.trim_start();
        if indent == 0 {
            let before_comment = body.split('#').next().unwrap_or("").trim_end();
            if before_comment == "results: []" || before_comment == "results:[]" {
                explicit_empty = true;
                continue;
            }
        }
        if body == "results:" && indent == 0 {
            in_results = true;
            continue;
        }
        if in_results && indent == 0 && !body.is_empty() {
            in_results = false;
            if let Some(g) = current_group.take() {
                groups.push(g);
            }
            continue;
        }
        if !in_results {
            continue;
        }
        if body.starts_with("- atoms:") {
            if let Some(g) = current_group.take() {
                groups.push(g);
            }
            // Quote-aware bracket pair finder: skips `[` and `]` that appear
            // inside `"..."` or `'...'` quoted strings. Necessary for atoms
            // whose error messages contain literal `[...]` substrings
            // (e.g. `Usage: (progn expr1 expr2 [...])`).
            let bracket = find_outer_bracket(body);
            if let Some((lo, hi)) = bracket {
                let inner = &body[lo + 1..hi];
                let atoms = split_yaml_list_inner(inner);
                current_group = Some(atoms);
            } else {
                current_group = Some(Vec::new());
            }
        }
    }
    if let Some(g) = current_group {
        groups.push(g);
    }
    if groups.is_empty() && !explicit_empty {
        None
    } else {
        Some(groups)
    }
}

/// Run a single fixture on T0 and compare canonicalized output to the
/// `.expected.yaml`'s `results:` block.
fn run_fixture(metta_path: &Path, yaml_path: &Path) -> Result<FixtureOutcome, String> {
    let source = fs::read_to_string(metta_path)
        .map_err(|e| format!("Cannot read {}: {}", metta_path.display(), e))?;
    let state = compile(&source).map_err(|e| format!("Compile error: {}", e))?;
    let mut env = new_env();
    let mut all: Vec<MettaValue> = Vec::new();
    let exprs: Vec<MettaValue> = state.source().iter().copied().collect();
    for expr in exprs {
        // Inc-6 GC root contract: the `all` accumulator holds result values from
        // PRIOR directives that are live until `canonicalize(&all)` below, but
        // they live in this Rust-local Vec, invisible to `collect_all_roots()`.
        // Register them as temporary roots so the single-threaded index
        // collector (which fires at each `eval_with_tier` quiescence point) does
        // not reclaim them. The handle refreshes each iteration and drops at
        // loop end. No-op cost in the slab build. See
        // docs/cesk-gc/single-threaded-collector.md.
        let _accum_roots = mettatron::backend::models::register_temporary_roots(all.clone());
        let outcome = eval_with_tier(
            expr,
            env,
            &state,
            TierSelection::Treewalker,
            FallbackPolicy::SilentDemote,
        );
        let (results, new_env) = match outcome {
            TierEvalOutcome::Ok { results, env, .. } => (results, env),
            TierEvalOutcome::Demoted { results, env, .. } => (results, env),
            TierEvalOutcome::NotApplicable { reason } => {
                return Err(format!("Tier not applicable: {:?}", reason));
            }
        };
        env = new_env;
        let is_bang = expr
            .as_sexpr()
            .and_then(|items| items.first())
            .and_then(|h| h.as_atom())
            .is_some_and(|s| s == "!");
        if is_bang {
            all.extend(results.into_iter().filter(|v| !v.is_empty()));
        }
    }
    let actual = canonicalize(&all);

    if let Some(status) = read_status(yaml_path) {
        if status == "host-dependent" {
            let expected_kinds: Vec<String> = parse_expected_results(yaml_path)
                .map(|groups| {
                    groups
                        .into_iter()
                        .flat_map(|g| g.into_iter())
                        .map(|s| atom_kind_prefix(&strip_yaml_quotes(s)))
                        .filter(|s| s.starts_with('('))
                        .collect()
                })
                .unwrap_or_default();
            let actual_kinds: Vec<String> = actual
                .iter()
                .map(|s| atom_kind_prefix(s))
                .filter(|s| s.starts_with('('))
                .collect();
            let kinds_match = if expected_kinds.is_empty() {
                actual.iter().any(|s| s.starts_with("(Error"))
            } else {
                let mut e = expected_kinds.clone();
                let mut a = actual_kinds.clone();
                e.sort();
                a.sort();
                e == a
            };
            return Ok(if kinds_match {
                FixtureOutcome::HostDependent
            } else {
                FixtureOutcome::Mismatch {
                    expected: if expected_kinds.is_empty() {
                        vec!["<any Error atom>".to_string()]
                    } else {
                        expected_kinds
                    },
                    actual: actual_kinds,
                }
            });
        }
    }

    let expected_groups = match parse_expected_results(yaml_path) {
        Some(g) => g,
        None => return Ok(FixtureOutcome::Skipped),
    };
    let mut expected: Vec<String> = expected_groups
        .into_iter()
        .flat_map(|g| g.into_iter())
        .map(strip_yaml_quotes)
        .collect();
    expected.sort();

    let is_xfail = read_status(yaml_path).as_deref() == Some("xfail");
    if is_xfail {
        return Ok(if expected != actual {
            FixtureOutcome::XFailExpected
        } else {
            FixtureOutcome::XPassUnexpected { expected }
        });
    }

    if expected == actual {
        Ok(FixtureOutcome::Pass)
    } else {
        Ok(FixtureOutcome::Mismatch { expected, actual })
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let options: BaseOptions = match parse_base_args(&args) {
        Ok(o) => o,
        Err(e) if e == "__help__" => {
            print_usage();
            process::exit(0);
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            print_usage();
            process::exit(1);
        }
    };

    if !options.conformance_dir.is_dir() {
        eprintln!(
            "Error: conformance directory not found: {}",
            options.conformance_dir.display()
        );
        process::exit(5);
    }

    // Discovery: prefer module-filter walk if specified; otherwise walk all
    // top-level subdirs. Fixture-name filter is applied as a post-pass.
    let mut fixtures = if options.module_filter.is_some() {
        discover_fixtures(&options.conformance_dir, options.module_filter.as_deref())
    } else {
        // No module filter: walk everything under the conformance root.
        let mut all = Vec::new();
        collect_fixtures_recursive(&options.conformance_dir, &mut all);
        all.sort();
        all
    };

    if let Some(needle) = &options.fixture_filter {
        fixtures.retain(|(m, _)| m.to_string_lossy().contains(needle));
    }

    if fixtures.is_empty() {
        eprintln!(
            "No fixtures found under {} (module filter: {:?}, fixture filter: {:?})",
            options.conformance_dir.display(),
            options.module_filter,
            options.fixture_filter,
        );
        process::exit(5);
    }

    eprintln!("Found {} fixture(s)", fixtures.len());
    let mut passes = 0usize;
    let mut failures = 0usize;
    let mut errors = 0usize;
    let mut skipped = 0usize;

    for (metta_path, yaml_path) in &fixtures {
        let rel = metta_path
            .strip_prefix(&options.conformance_dir)
            .unwrap_or(metta_path);
        match run_fixture(metta_path, yaml_path) {
            Ok(FixtureOutcome::Pass) => {
                println!("{}: PASS", rel.display());
                passes += 1;
            }
            Ok(FixtureOutcome::Mismatch { expected, actual }) => {
                println!(
                    "{}: FAIL\n  expected: {:?}\n  actual:   {:?}",
                    rel.display(),
                    expected,
                    actual
                );
                failures += 1;
            }
            Ok(FixtureOutcome::Skipped) => {
                println!("{}: SKIP (no expected results in yaml)", rel.display());
                skipped += 1;
            }
            Ok(FixtureOutcome::HostDependent) => {
                println!(
                    "{}: PASS (host-dependent — accepted any Error)",
                    rel.display()
                );
                passes += 1;
            }
            Ok(FixtureOutcome::XFailExpected) => {
                println!("{}: XFAIL (expected-to-fail, confirmed)", rel.display());
                passes += 1;
            }
            Ok(FixtureOutcome::XPassUnexpected { expected }) => {
                println!(
                    "{}: XPASS (status: xfail but actual matches expected = {:?}; clear xfail status)",
                    rel.display(),
                    expected
                );
                passes += 1;
            }
            Err(e) => {
                eprintln!("{}: ERROR — {}", rel.display(), e);
                errors += 1;
            }
        }
    }

    println!();
    println!(
        "Summary: {} pass, {} fail, {} error, {} skipped",
        passes, failures, errors, skipped
    );

    // Inc-6 single-threaded index GC observability: when requested, report how
    // many live mark+sweep cycles fired during the run. A vacuous trigger leaves
    // this at 0 (so validation can confirm the collector actually ran).
    if std::env::var("METTATRON_INDEX_GC_REPORT").as_deref() == Ok("1") {
        let cycles = mettatron::backend::eval::cesk::index_heap::index_gc::cycles_run();
        let midloop = mettatron::backend::eval::cesk::index_heap::index_gc::midloop_cycles_run();
        eprintln!("INDEX_GC_CYCLES_RUN={cycles} INDEX_GC_MIDLOOP_CYCLES={midloop}");
        // Increment B observability: final heap state, to explain WHY the trigger did/
        // didn't fire (e.g. cycles=0 because the run's young allocation never crossed
        // YOUNG_BUDGET=2 MiB, or old_live stayed 0 on a single-segment workload where
        // promote is a no-op). committed = node-slab + side-spine pointers.
        {
            let h = mettatron::backend::eval::cesk::index_heap::global_index_heap()
                .read()
                .expect("index heap");
            eprintln!(
                "INDEX_GC_FINAL committed={} young_alloc={} old_live={} live={}",
                h.committed_bytes(),
                h.young_alloc_bytes(),
                h.old_live_bytes(),
                h.live_bytes()
            );
        }
    }

    if (failures > 0 || errors > 0) && options.strict {
        process::exit(3);
    }
    process::exit(0);
}
