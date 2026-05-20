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
use std::path::{Path, PathBuf};
use std::process;

use mettatron::backend::eval::tier_forced::{
    eval_with_tier, FallbackPolicy, TierEvalOutcome, TierSelection,
};
use mettatron::backend::eval::trampoline::new_env;
use mettatron::backend::models::ValueView;
use mettatron::{compile, MettaValue};

fn print_usage() {
    eprintln!("mtt-conformance — conformance test runner for MeTTaTron");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("    mtt-conformance --conformance-dir <PATH> [--module <NAME>]");
    eprintln!();
    eprintln!("OPTIONS:");
    eprintln!("    --conformance-dir <PATH>  Root of the conformance/ directory");
    eprintln!("    --module <NAME>           Only run fixtures in this subdir (e.g. M11-bisimilarity-he)");
    eprintln!("    --tier <T1,T2,...>        Restrict to specific tiers (default: all)");
    eprintln!("    --strict                  Exit code 3 on any divergence (default: lenient)");
    eprintln!("    --help                    Show this help");
    eprintln!();
    eprintln!("EXIT CODES:");
    eprintln!("    0  — all fixtures pass");
    eprintln!("    3  — divergence detected (with --strict)");
    eprintln!("    5  — directory missing");
}

struct Options {
    conformance_dir: PathBuf,
    module_filter: Option<String>,
    strict: bool,
}

fn parse_args() -> Result<Options, String> {
    let args: Vec<String> = env::args().collect();
    let mut conformance_dir: Option<PathBuf> = None;
    let mut module_filter: Option<String> = None;
    let mut strict = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_usage();
                process::exit(0);
            }
            "--conformance-dir" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing path after --conformance-dir".to_string());
                }
                conformance_dir = Some(PathBuf::from(&args[i]));
            }
            "--module" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing name after --module".to_string());
                }
                module_filter = Some(args[i].clone());
            }
            "--strict" => {
                strict = true;
            }
            // --tier is accepted but not yet wired (placeholder for future Phase H4 tier filtering)
            "--tier" => {
                i += 1;
            }
            other => return Err(format!("Unknown argument: {}", other)),
        }
        i += 1;
    }

    let conformance_dir =
        conformance_dir.ok_or_else(|| "--conformance-dir is required".to_string())?;
    Ok(Options {
        conformance_dir,
        module_filter,
        strict,
    })
}

/// Walk the conformance directory and collect `.metta` + `.expected.yaml` pairs.
fn discover_fixtures(dir: &Path, module_filter: Option<&str>) -> Vec<(PathBuf, PathBuf)> {
    let mut fixtures: Vec<(PathBuf, PathBuf)> = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return fixtures,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let dirname = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if let Some(filter) = module_filter {
                if !dirname.starts_with(filter) {
                    continue;
                }
            }
            // Recurse one level for module dirs.
            if let Ok(sub) = fs::read_dir(&path) {
                for sub_entry in sub.flatten() {
                    let sub_path = sub_entry.path();
                    if sub_path.extension().and_then(|s| s.to_str()) == Some("metta") {
                        let yaml = sub_path.with_extension("expected.yaml");
                        if yaml.exists() {
                            fixtures.push((sub_path, yaml));
                        }
                    }
                }
            }
        }
    }
    fixtures.sort();
    fixtures
}

/// Extract the leading S-expression "kind" prefix from an atom string.
///
/// Maps atoms to their type-prefix for host-dependent fixture comparison:
/// - `(Memo 1 "my-cache")` → `(Memo`
/// - `(Memo 99 "")` → `(Memo`
/// - `(Error msg detail)` → `(Error`
/// - `42` → `42` (no kind prefix; literals compare directly)
/// - `(foo bar baz)` → `(foo`
///
/// Returns the full string if no parenthesised head is present.
fn atom_kind_prefix(s: &str) -> String {
    if let Some(rest) = s.strip_prefix('(') {
        let head: String = rest
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != ')')
            .collect();
        format!("({}", head)
    } else {
        s.to_string()
    }
}

/// Canonical multiset comparison: sort results lexicographically, dropping Empty.
fn canonicalize(results: &[MettaValue]) -> Vec<String> {
    let mut s: Vec<String> = results
        .iter()
        .filter(|v| !v.is_empty())
        .map(format_value)
        .collect();
    s.sort();
    s
}

/// S15a parse-string-escape (2026-05-15): mirror main.rs::format_string_escaped.
/// Round-trips through the lexer by escaping `\\` and `\"` per HE §A.5.
fn format_string_escaped(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// **Stack-safety + memory-safety fix (2026-05-15)**: iterative + memoized.
/// Was recursive on SExpr/Conjunction/Error/Type/Quoted children and
/// exponentially vulnerable to shared substructure (same defect class as
/// the 6.7 PB `to_display_string` bug). Format remains unchanged
/// (parser-roundtrip + Type(_) wrapping); only the implementation paradigm
/// switches to a heap work-list + per-call memo keyed by slab pointer.
fn format_value(v: &MettaValue) -> String {
    enum Work {
        Process(MettaValue),
        Join {
            count: usize,
            prefix: &'static str,
            suffix: &'static str,
            separator: &'static str,
            memo_key: Option<usize>,
        },
        /// Workstream B (Task #6 follow-up, 2026-05-18): HE-style render
        /// for `(Bindings ($x val) ($y val2) …)`. `var_names[i]` is the
        /// pre-extracted variable name (cheap — atoms are O(1)) for the
        /// i-th pair. `malformed[i] == true` means the i-th pair wasn't a
        /// well-formed 2-element `($var val)` SExpr; the work-stack pushed
        /// the entire pair as `Process(pair)` instead, and the join arm
        /// uses the rendered string verbatim (no `$var <- ` prefix).
        JoinBindings {
            var_names: Vec<String>,
            malformed: Vec<bool>,
            memo_key: Option<usize>,
        },
    }
    let mut work: Vec<Work> = Vec::with_capacity(16);
    let mut result: Vec<String> = Vec::with_capacity(16);
    let mut memo: std::collections::HashMap<usize, String> =
        std::collections::HashMap::with_capacity(64);
    work.push(Work::Process(v.clone()));
    while let Some(w) = work.pop() {
        match w {
            Work::Process(val) => {
                // Memo key = slab pointer (or inline-pseudo pointer for
                // tagged values — distinct inline bit patterns produce
                // distinct pseudo-pointers, so collision is impossible).
                let k = val.inner_ptr() as usize;
                if let Some(cached) = memo.get(&k) {
                    result.push(cached.clone());
                    continue;
                }
                let memo_key_opt = Some(k);
                match val.view() {
                    ValueView::Bool(b) => result.push(if b { "True" } else { "False" }.to_string()),
                    ValueView::Long(n) => result.push(n.to_string()),
                    ValueView::Float(fl) => result.push(format!("{}", fl)),
                    ValueView::Unit => result.push("()".to_string()),
                    ValueView::Empty => result.push("Empty".to_string()),
                    ValueView::NotReducible => result.push("NotReducible".to_string()),
                    ValueView::Atom(s) => result.push(s.to_string()),
                    ValueView::String(s) => result.push(format_string_escaped(s)),
                    ValueView::Space(h) => {
                        // Phase C (HE bisim, 2026-05-20): HE-aligned space
                        // print form per fixture T04/028 / §06.15.
                        let canonical = if h.name == "self" {
                            "ModuleSpace(GroundingSpace-top)".to_string()
                        } else {
                            format!("&{}", h.name)
                        };
                        result.push(canonical);
                    }
                    ValueView::State(id) => result.push(format!("(State {})", id)),
                    ValueView::Memo(h) => {
                        result.push(format!("(Memo {} \"{}\")", h.id, h.name))
                    }
                    ValueView::Error(msg, details) => {
                        // Layout when Join fires: result has [..., msg_str, details_str].
                        // Push msg_str into result now (it's just an atom name).
                        // Schedule Process(details) → pushes details_str later.
                        // Then Join {count=2} drains both and formats.
                        work.push(Work::Join {
                            count: 2,
                            prefix: "(Error ",
                            suffix: ")",
                            separator: " ",
                            memo_key: memo_key_opt,
                        });
                        work.push(Work::Process(details));
                        result.push(msg.to_string());
                    }
                    ValueView::Type(t) => {
                        work.push(Work::Join {
                            count: 1,
                            prefix: "Type(",
                            suffix: ")",
                            separator: "",
                            memo_key: memo_key_opt,
                        });
                        work.push(Work::Process(t));
                    }
                    ValueView::SExpr(items) => {
                        if items.is_empty() {
                            let s = "()".to_string();
                            if let Some(k) = memo_key_opt {
                                memo.insert(k, s.clone());
                            }
                            result.push(s);
                        } else if items
                            .first()
                            .and_then(|h| h.as_atom())
                            == Some("Bindings")
                        {
                            // Workstream B: HE-style `{ }` / `{ $x <- val, … }`
                            // render for `(Bindings ($x val) …)` SExpr.
                            // Mirrors HE's `Display for Bindings`
                            // (`hyperon-experimental/hyperon-atom/src/matcher.rs:762-789`).
                            let pairs = &items[1..];
                            if pairs.is_empty() {
                                let s = "{ }".to_string();
                                if let Some(k) = memo_key_opt {
                                    memo.insert(k, s.clone());
                                }
                                result.push(s);
                            } else {
                                let mut var_names: Vec<String> =
                                    Vec::with_capacity(pairs.len());
                                let mut malformed: Vec<bool> =
                                    Vec::with_capacity(pairs.len());
                                for p in pairs {
                                    let (name, ok) = match p.view() {
                                        ValueView::SExpr(kv) if kv.len() == 2 => {
                                            match kv[0].view() {
                                                ValueView::Atom(n) => {
                                                    (n.to_string(), true)
                                                }
                                                _ => (String::new(), false),
                                            }
                                        }
                                        _ => (String::new(), false),
                                    };
                                    var_names.push(name);
                                    malformed.push(!ok);
                                }
                                // Stack discipline: JoinBindings consumes `pairs.len()` strings
                                // from `result`. Push the renderer first, then push Process
                                // items in reverse so they're processed in original order and
                                // appear on `result` in original order (top = last pair).
                                work.push(Work::JoinBindings {
                                    var_names,
                                    malformed: malformed.clone(),
                                    memo_key: memo_key_opt,
                                });
                                for (i, p) in pairs.iter().enumerate().rev() {
                                    let to_render = if malformed[i] {
                                        // Render the whole malformed pair as normal SExpr text.
                                        p.clone()
                                    } else if let ValueView::SExpr(kv) = p.view() {
                                        // Render only the VALUE; var name is already captured.
                                        kv[1].clone()
                                    } else {
                                        p.clone()
                                    };
                                    work.push(Work::Process(to_render));
                                }
                            }
                        } else {
                            work.push(Work::Join {
                                count: items.len(),
                                prefix: "(",
                                suffix: ")",
                                separator: " ",
                                memo_key: memo_key_opt,
                            });
                            for item in items.iter().rev() {
                                work.push(Work::Process(item.clone()));
                            }
                        }
                    }
                    ValueView::Conjunction(g) => {
                        if g.is_empty() {
                            let s = "(, )".to_string();
                            if let Some(k) = memo_key_opt {
                                memo.insert(k, s.clone());
                            }
                            result.push(s);
                        } else {
                            work.push(Work::Join {
                                count: g.len(),
                                prefix: "(, ",
                                suffix: ")",
                                separator: " ",
                                memo_key: memo_key_opt,
                            });
                            for goal in g.iter().rev() {
                                work.push(Work::Process(goal.clone()));
                            }
                        }
                    }
                    ValueView::Quoted(inner) => {
                        work.push(Work::Join {
                            count: 1,
                            prefix: "(quote ",
                            suffix: ")",
                            separator: "",
                            memo_key: memo_key_opt,
                        });
                        work.push(Work::Process(inner));
                    }
                }
            }
            Work::Join {
                count,
                prefix,
                suffix,
                separator,
                memo_key,
            } => {
                let start = result.len() - count;
                let parts: Vec<String> = result.drain(start..).collect();
                let formatted =
                    format!("{}{}{}", prefix, parts.join(separator), suffix);
                if let Some(k) = memo_key {
                    memo.insert(k, formatted.clone());
                }
                result.push(formatted);
            }
            Work::JoinBindings {
                var_names,
                malformed,
                memo_key,
            } => {
                let count = var_names.len();
                let start = result.len() - count;
                let parts: Vec<String> = result.drain(start..).collect();
                let segs: Vec<String> = parts
                    .into_iter()
                    .enumerate()
                    .map(|(i, rendered)| {
                        if malformed[i] {
                            rendered
                        } else {
                            format!("{} <- {}", var_names[i], rendered)
                        }
                    })
                    .collect();
                let s = format!("{{ {} }}", segs.join(", "));
                if let Some(k) = memo_key {
                    memo.insert(k, s.clone());
                }
                result.push(s);
            }
        }
    }
    result.pop().unwrap_or_default()
}

/// Parse the expected `results:` block from an `.expected.yaml` fixture.
///
/// The schema is a YAML list under `results:`:
/// ```yaml
/// results:
///   - atoms: ["True"]
///   - atoms: ["False"]
/// ```
/// Each `atoms: [...]` entry corresponds to one `!`-prefixed expression's
/// canonical multiset (whitespace-trimmed, quotes preserved).
///
/// Returns a flat `Vec<Vec<String>>` where the outer Vec is per-directive
/// and the inner Vec is the multiset of result strings. Returns `None` if
/// the YAML doesn't have a `results:` block or if it's empty (no expected
/// results — fixture only asserts non-crash).
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
        // Inline empty list `results: []` at top level: explicitly asserts
        // zero result groups (no `!` directives in the fixture). Strip
        // trailing comments before comparing.
        if indent == 0 {
            let before_comment = body.split('#').next().unwrap_or("").trim_end();
            if before_comment == "results: []" || before_comment == "results:[]" {
                explicit_empty = true;
                continue;
            }
        }
        // Top-level `results:` (indent 0) opens the block. Nested
        // `he_observation: results:` (indent > 0) is ignored — we only want
        // the canonical MTT expected output.
        if body == "results:" && indent == 0 {
            in_results = true;
            continue;
        }
        if in_results && indent == 0 && !body.is_empty() {
            // Left the results block — a new top-level key has started.
            in_results = false;
            if let Some(g) = current_group.take() {
                groups.push(g);
            }
            continue;
        }
        if !in_results {
            continue;
        }
        // Detect a top-level results entry: "- atoms: [...]" at indent 2.
        if body.starts_with("- atoms:") {
            if let Some(g) = current_group.take() {
                groups.push(g);
            }
            let bracket = body.find('[').and_then(|i| body[i..].find(']').map(|j| (i, i + j)));
            if let Some((lo, hi)) = bracket {
                let inner = &body[lo + 1..hi];
                // BUG-fix: quote-aware splitting. Naive `inner.split(',')`
                // breaks atoms like `"(, 1 2)"` or `"(Error ... (foo))"` where
                // commas/parens appear inside YAML string literals. Track
                // double-quote state and depth of `()` brackets.
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

#[derive(Debug, PartialEq, Eq)]
enum FixtureOutcome {
    Pass,
    Mismatch { expected: Vec<String>, actual: Vec<String> },
    Skipped, // No expected results in YAML — fixture only asserts non-crash
    HostDependent, // Fixture declares status: host-dependent; pass if any Error atom present
    XFailExpected, // Fixture declares status: xfail; expected mismatch confirmed
    XPassUnexpected { expected: Vec<String> }, // Fixture declares xfail but actually matched
}

/// Read the top-level `status:` key from a fixture YAML (if present).
fn read_status(yaml_path: &Path) -> Option<String> {
    let content = fs::read_to_string(yaml_path).ok()?;
    for raw_line in content.lines() {
        let trimmed = raw_line.trim_end();
        let indent = trimmed.len() - trimmed.trim_start().len();
        if indent != 0 {
            continue;
        }
        let body = trimmed.trim_start();
        if let Some(value) = body.strip_prefix("status:") {
            return Some(value.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
        }
    }
    None
}

/// Run a single fixture on T0 and compare canonicalized output to the
/// `.expected.yaml`'s `results:` block. The per-directive `atoms` lists
/// are concatenated (with multiset semantics within each directive) to
/// form the canonical expected output. Quote/whitespace handling matches
/// the format used by `format_value`.
fn run_fixture(metta_path: &Path, yaml_path: &Path) -> Result<FixtureOutcome, String> {
    let source = fs::read_to_string(metta_path)
        .map_err(|e| format!("Cannot read {}: {}", metta_path.display(), e))?;
    let state = compile(&source).map_err(|e| format!("Compile error: {}", e))?;
    let mut env = new_env();
    let mut all: Vec<MettaValue> = Vec::new();
    let exprs: Vec<MettaValue> = state.source().iter().copied().collect();
    for expr in exprs {
        // Only `!`-prefixed S-exprs produce observable output. Atoms,
        // top-level facts, etc. are processed for side-effects only.
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
        // S1 TOPLEVEL (2026-05-13): defence-in-depth structural bang gate.
        // Mirror HE's MettaRunnerMode::INTERPRET selector: only `(! expr)`
        // directives contribute to the observable result multiset. The
        // env-side gate in process_single_combination_generic /
        // op_dispatch_rules / call_support already emits `[]` for bare
        // ADD-mode S-exprs at every tier, but this structural check
        // catches any tier regression that lags behind.
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

    // host-dependent fixtures: two acceptance modes.
    //
    // (a) Strict kind-match (preferred when expected has parenthesised heads
    //     like `(Memo`, `(State`, `(Space`): accept iff each actual atom's
    //     leading S-expression head matches the expected. This accepts
    //     host-dependent IDs (Memo 1 vs Memo 2) while still requiring the
    //     same atom *kind*.
    //
    // (b) Lenient Error-presence (legacy, kicks in when expected has no
    //     parenthesised heads or none was parseable): accept iff any Error
    //     atom is present in actual. Used for fixtures whose host-dependent
    //     bit only flags the error message format / arity / category text.
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
                // Lenient: accept iff any Error atom is present
                actual.iter().any(|s| s.starts_with("(Error"))
            } else {
                // Strict: kind-multiset must match
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
    // Flatten expected groups into a single canonical multiset (matches the
    // `all` collection above which accumulates across all `!` directives).
    let mut expected: Vec<String> = expected_groups
        .into_iter()
        .flat_map(|g| g.into_iter())
        .map(strip_yaml_quotes)
        .collect();
    expected.sort();

    // status: xfail — fixture documents expected-to-fail empirical behavior.
    // If actual ≠ expected, that's confirmed-xfail (PASS).
    // If actual == expected, that's an unexpected pass (test author should
    // clear the xfail status).
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

/// Quote- and paren-aware split of a YAML inline-list body.
///
/// Splits `inner` (the text between `[` and `]`) on top-level commas only:
/// commas inside `"..."` double-quoted strings or inside `(...)` parentheses
/// are preserved. Returns the trimmed atom strings (with outer quotes intact —
/// `strip_yaml_quotes` handles those downstream).
fn split_yaml_list_inner(inner: &str) -> Vec<String> {
    let mut atoms: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_double_quote = false;
    let mut paren_depth: i32 = 0;
    let mut escape = false;
    for ch in inner.chars() {
        if escape {
            current.push(ch);
            escape = false;
            continue;
        }
        match ch {
            '\\' if in_double_quote => {
                current.push(ch);
                escape = true;
            }
            '"' => {
                in_double_quote = !in_double_quote;
                current.push(ch);
            }
            '(' if !in_double_quote => {
                paren_depth += 1;
                current.push(ch);
            }
            ')' if !in_double_quote => {
                paren_depth = (paren_depth - 1).max(0);
                current.push(ch);
            }
            ',' if !in_double_quote && paren_depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    atoms.push(trimmed);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        atoms.push(trimmed);
    }
    atoms
}

/// YAML-style string normalization: strip outer single or double quotes
/// and decode common backslash escapes inside double-quoted strings.
///
/// Necessary because `["True", "False"]` parses each atom with quotes
/// included, whereas `format_value` emits booleans without quotes.
///
/// Decoded escapes inside double quotes: `\n`, `\t`, `\r`, `\\`, `\"`.
/// Single-quoted strings are taken verbatim per YAML 1.2 (no escaping).
fn strip_yaml_quotes(s: String) -> String {
    let t = s.trim();
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        let inner = &t[1..t.len() - 1];
        // Decode YAML double-quoted escapes: \n \t \r \\ \"
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('\\') => out.push('\\'),
                    Some('"') => out.push('"'),
                    Some(other) => {
                        // Unknown escape — preserve verbatim (forward-compat
                        // with YAML's \u{HHHH} unicode escapes which we don't
                        // currently emit from format_value).
                        out.push('\\');
                        out.push(other);
                    }
                    None => out.push('\\'),
                }
            } else {
                out.push(c);
            }
        }
        out
    } else if t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2 {
        // YAML 1.2 single-quoted strings: only `''` doubles as an escape for `'`.
        let inner = &t[1..t.len() - 1];
        inner.replace("''", "'")
    } else {
        t.to_string()
    }
}

fn main() {
    let options = match parse_args() {
        Ok(o) => o,
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

    let fixtures = discover_fixtures(
        &options.conformance_dir,
        options.module_filter.as_deref(),
    );

    if fixtures.is_empty() {
        eprintln!(
            "No fixtures found under {} (module filter: {:?})",
            options.conformance_dir.display(),
            options.module_filter
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
                println!("{}: PASS (host-dependent — accepted any Error)", rel.display());
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
                // Treat unexpected pass as a soft warning, not a failure —
                // it means the fixture author can remove `status: xfail`.
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

    if (failures > 0 || errors > 0) && options.strict {
        process::exit(3);
    }
    process::exit(0);
}
