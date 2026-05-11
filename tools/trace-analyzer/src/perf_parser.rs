// perf_parser.rs — Parse folded perf stacks (output of `perf script | stackcollapse-perf.pl`).
//
// Input format: one line per unique stack trace, `frame1;frame2;...;frameN COUNT`
// where COUNT is the number of samples with that exact stack.

use std::collections::HashMap;
use std::fs;

use crate::function_map::extract_leaf_name;

/// Aggregated CPU profile from folded perf stacks.
pub struct PerfProfile {
    /// Per-function profiles, sorted by self_samples descending.
    pub functions: Vec<PerfFunctionProfile>,
    /// Total number of samples across all stacks.
    pub total_samples: u64,
}

/// CPU profile for a single (demangled leaf) function name.
pub struct PerfFunctionProfile {
    /// Demangled leaf function name.
    pub function_name: String,
    /// Samples where this function is the top of stack (direct CPU time).
    pub self_samples: u64,
    /// Samples where this function appears anywhere in the stack (inclusive time).
    pub inclusive_samples: u64,
}

/// Strip Rust monomorphization hash suffix `::h[0-9a-f]{16}`.
fn strip_hash_suffix(name: &str) -> &str {
    if let Some(idx) = name.rfind("::h") {
        let suffix = &name[idx + 3..];
        if suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
            return &name[..idx];
        }
    }
    name
}

/// Normalize a frame name: strip hash suffix, extract leaf, strip angle-bracket generics.
fn normalize_frame(raw: &str) -> String {
    let no_hash = strip_hash_suffix(raw);
    let leaf = extract_leaf_name(no_hash);

    // Strip <...> generics from the leaf
    if let Some(start) = leaf.find('<') {
        leaf[..start].to_string()
    } else {
        leaf.to_string()
    }
}

/// Parse a folded-stacks file into a `PerfProfile`.
///
/// Expected input: output of `perf script | stackcollapse-perf.pl` or `perf script -F folded`.
/// Each line: `frame1;frame2;...;frameN COUNT` (last whitespace separates stack from count).
///
/// Lines starting with `#` are skipped (comments/headers).
/// Empty lines are skipped.
pub fn parse_folded_stacks(path: &str) -> Result<PerfProfile, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read folded stacks file '{}': {}", path, e))?;

    if content.trim().is_empty() {
        return Err(format!("Folded stacks file '{}' is empty", path));
    }

    // (self_samples, inclusive_samples) per normalized function name
    let mut map: HashMap<String, (u64, u64)> = HashMap::new();
    let mut total_samples: u64 = 0;
    let mut line_num: usize = 0;

    for line in content.lines() {
        line_num += 1;
        let line = line.trim();

        // Skip empty and comment lines
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // Split on last whitespace → (stack_chain, count)
        let (stack_chain, count_str) = match line.rfind(|c: char| c.is_ascii_whitespace()) {
            Some(idx) => (&line[..idx], line[idx..].trim()),
            None => {
                // Malformed line — skip with warning
                eprintln!(
                    "Warning: line {} has no sample count, skipping: {}",
                    line_num,
                    &line[..line.len().min(80)]
                );
                continue;
            }
        };

        let count: u64 = match count_str.parse() {
            Ok(c) => c,
            Err(_) => {
                eprintln!(
                    "Warning: line {} has non-numeric count '{}', skipping",
                    line_num, count_str
                );
                continue;
            }
        };

        total_samples += count;

        // Split stack chain on `;`
        let frames: Vec<&str> = stack_chain.split(';').collect();
        if frames.is_empty() {
            continue;
        }

        // Track which normalized names we've already counted for inclusive in this stack
        // (avoid double-counting recursive functions).
        let mut seen_in_stack: HashMap<String, bool> = HashMap::new();

        for (i, raw_frame) in frames.iter().enumerate() {
            let norm = normalize_frame(raw_frame);
            if norm.is_empty() {
                continue;
            }

            let is_top = i == frames.len() - 1;

            let entry = map.entry(norm.clone()).or_insert((0, 0));

            // Self samples: only for the top-of-stack frame
            if is_top {
                entry.0 += count;
            }

            // Inclusive samples: once per unique function per stack
            if !seen_in_stack.contains_key(&norm) {
                entry.1 += count;
                seen_in_stack.insert(norm, true);
            }
        }
    }

    if total_samples == 0 {
        return Err(format!(
            "No valid samples found in folded stacks file '{}'",
            path
        ));
    }

    // Convert to sorted Vec
    let mut functions: Vec<PerfFunctionProfile> = map
        .into_iter()
        .map(|(name, (self_s, incl_s))| PerfFunctionProfile {
            function_name: name,
            self_samples: self_s,
            inclusive_samples: incl_s,
        })
        .collect();

    // Sort by self_samples descending, then by name for stability
    functions.sort_by(|a, b| {
        b.self_samples
            .cmp(&a.self_samples)
            .then_with(|| a.function_name.cmp(&b.function_name))
    });

    Ok(PerfProfile {
        functions,
        total_samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn write_temp_file(content: &str) -> String {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = format!("/tmp/test_perf_folded_{}_{}.txt", std::process::id(), id);
        let mut f = fs::File::create(&path).expect("create temp file");
        f.write_all(content.as_bytes()).expect("write temp file");
        path
    }

    #[test]
    fn test_normalize_frame() {
        assert_eq!(
            normalize_frame("mettatron::backend::eval::eval_inner::h0123456789abcdef"),
            "eval_inner"
        );
        assert_eq!(
            normalize_frame("HashMap<String, Vec<u64>>::insert"),
            "insert"
        );
        assert_eq!(normalize_frame("main"), "main");
    }

    #[test]
    fn test_strip_hash_suffix() {
        assert_eq!(
            strip_hash_suffix("eval_inner::h0123456789abcdef"),
            "eval_inner"
        );
        // Short hash — not stripped
        assert_eq!(strip_hash_suffix("eval_inner::habcd"), "eval_inner::habcd");
    }

    #[test]
    fn test_parse_simple_folded_stacks() {
        let content = "\
main;eval_trampoline;eval_inner 100
main;eval_trampoline;match_rules_native 50
main;eval_trampoline;apply_bindings 30
";
        let path = write_temp_file(content);
        let profile = parse_folded_stacks(&path).expect("parse should succeed");
        fs::remove_file(&path).ok();

        assert_eq!(profile.total_samples, 180);

        // eval_inner: 100 self, 100 inclusive
        let eval_inner = profile
            .functions
            .iter()
            .find(|f| f.function_name == "eval_inner")
            .expect("eval_inner should exist");
        assert_eq!(eval_inner.self_samples, 100);
        assert_eq!(eval_inner.inclusive_samples, 100);

        // eval_trampoline: 0 self (never top), 180 inclusive (in all stacks)
        let trampoline = profile
            .functions
            .iter()
            .find(|f| f.function_name == "eval_trampoline")
            .expect("eval_trampoline should exist");
        assert_eq!(trampoline.self_samples, 0);
        assert_eq!(trampoline.inclusive_samples, 180);
    }

    #[test]
    fn test_comments_and_empty_lines() {
        let content = "\
# This is a comment
main;eval_inner 10

# Another comment
main;match_rules 5
";
        let path = write_temp_file(content);
        let profile = parse_folded_stacks(&path).expect("parse should succeed");
        fs::remove_file(&path).ok();

        assert_eq!(profile.total_samples, 15);
        assert_eq!(profile.functions.len(), 3); // main, eval_inner, match_rules
    }

    #[test]
    fn test_recursive_function_inclusive_count() {
        // Recursive function appears twice in same stack
        let content = "main;eval_inner;eval_inner 20\n";
        let path = write_temp_file(content);
        let profile = parse_folded_stacks(&path).expect("parse should succeed");
        fs::remove_file(&path).ok();

        let eval_inner = profile
            .functions
            .iter()
            .find(|f| f.function_name == "eval_inner")
            .expect("eval_inner should exist");
        // Self: 20 (it's top of stack)
        assert_eq!(eval_inner.self_samples, 20);
        // Inclusive: 20 (counted once per stack, not twice)
        assert_eq!(eval_inner.inclusive_samples, 20);
    }

    #[test]
    fn test_empty_file_error() {
        let path = write_temp_file("");
        let result = parse_folded_stacks(&path);
        fs::remove_file(&path).ok();
        assert!(result.is_err());
    }

    #[test]
    fn test_sorted_by_self_samples() {
        let content = "\
main;a 10
main;b 30
main;c 20
";
        let path = write_temp_file(content);
        let profile = parse_folded_stacks(&path).expect("parse should succeed");
        fs::remove_file(&path).ok();

        // Top functions (excluding 'main' which has 0 self-samples) should be b, c, a
        let self_only: Vec<(&str, u64)> = profile
            .functions
            .iter()
            .filter(|f| f.self_samples > 0)
            .map(|f| (f.function_name.as_str(), f.self_samples))
            .collect();
        assert_eq!(self_only, vec![("b", 30), ("c", 20), ("a", 10)]);
    }
}
