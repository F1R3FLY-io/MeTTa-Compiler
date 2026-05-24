//! Shared utilities for the conformance test runners (`mtt-conformance`,
//! `petta-conformance`).
//!
//! This module is shared between binaries via `#[path = "conformance_common.rs"]
//! mod conformance_common;` at the top of each `src/bin/*_conformance.rs`.
//! It is NOT exposed through the library API — that would make conformance
//! plumbing part of the public mettatron interface.

#![allow(dead_code)]
//   ^ each binary uses a different subset; suppressing unused-warnings here
//     keeps the diff to mtt_conformance.rs minimal during the refactor.

use std::fs;
use std::path::{Path, PathBuf};

use mettatron::backend::models::ValueView;
use mettatron::MettaValue;

// ───────────────────────────────────────────────────────────────────────────
// Fixture discovery
// ───────────────────────────────────────────────────────────────────────────

/// Walk the conformance directory and collect `.metta` + `.expected.yaml`
/// pairs. Recurses into sub-buckets so both flat layouts
/// (`M11-bisimilarity-he/*.metta`) and hierarchical layouts
/// (`M11-bisimilarity-pt/3xx-translator-forms/*.metta`) are reached.
///
/// `module_filter` matches against top-level directory names by prefix
/// (e.g. filter `"M11-"` matches both `M11-bisimilarity-pt` and `M11-bisimilarity-he`).
pub fn discover_fixtures(dir: &Path, module_filter: Option<&str>) -> Vec<(PathBuf, PathBuf)> {
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
            collect_fixtures_recursive(&path, &mut fixtures);
        }
    }
    fixtures.sort();
    fixtures
}

/// Recursive helper for `discover_fixtures`. Public so binaries that need
/// flat-corpus walking (e.g. PeTTa's `P00..P21` sits directly under the
/// conformance root with no per-module top-level grouping) can call it
/// without going through the module-filter layer.
pub fn collect_fixtures_recursive(dir: &Path, fixtures: &mut Vec<(PathBuf, PathBuf)>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_fixtures_recursive(&path, fixtures);
        } else if path.extension().and_then(|s| s.to_str()) == Some("metta") {
            let yaml = path.with_extension("expected.yaml");
            if yaml.exists() {
                fixtures.push((path, yaml));
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Result canonicalization
// ───────────────────────────────────────────────────────────────────────────

/// Extract the leading S-expression "kind" prefix from an atom string.
///
/// Maps atoms to their type-prefix for host-dependent fixture comparison:
/// - `(Memo 1 "my-cache")` → `(Memo`
/// - `(Error msg detail)` → `(Error`
/// - `42` → `42` (no kind prefix; literals compare directly)
/// - `(foo bar baz)` → `(foo`
pub fn atom_kind_prefix(s: &str) -> String {
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
pub fn canonicalize(results: &[MettaValue]) -> Vec<String> {
    let mut s: Vec<String> = results
        .iter()
        .filter(|v| !v.is_empty())
        .map(format_value)
        .collect();
    s.sort();
    s
}

/// String-escape per HE §A.5 — escape `\\` and `\"` so the result round-trips
/// through the lexer.
pub fn format_string_escaped(s: &str) -> String {
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

/// Format a MettaValue for canonicalized comparison. Iterative + memoized to
/// avoid exponential blow-up on shared substructure (mirrors the fix to the
/// 6.7 PB `to_display_string` bug).
pub fn format_value(v: &MettaValue) -> String {
    enum Work {
        Process(MettaValue),
        Join {
            count: usize,
            prefix: &'static str,
            suffix: &'static str,
            separator: &'static str,
            memo_key: Option<usize>,
        },
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
                let k = val.inner_ptr() as usize;
                if let Some(cached) = memo.get(&k) {
                    result.push(cached.clone());
                    continue;
                }
                let memo_key_opt = Some(k);
                match val.view() {
                    ValueView::Bool(b) => {
                        // Phase 1.5 PT alignment (2026-05-22): lowercase per
                        // PHE-finer #10; matches user-visible Display output.
                        result.push(if b { "true" } else { "false" }.to_string())
                    }
                    ValueView::Long(n) => result.push(n.to_string()),
                    ValueView::Float(fl) => result.push(format!("{}", fl)),
                    ValueView::Unit => result.push("()".to_string()),
                    ValueView::Empty => result.push("Empty".to_string()),
                    ValueView::NotReducible => result.push("NotReducible".to_string()),
                    ValueView::Atom(s) => result.push(s.to_string()),
                    ValueView::String(s) => result.push(format_string_escaped(s)),
                    ValueView::Space(h) => {
                        let canonical = if h.name == "self" {
                            "ModuleSpace(GroundingSpace-top)".to_string()
                        } else {
                            format!("&{}", h.name)
                        };
                        result.push(canonical);
                    }
                    ValueView::State(id) => result.push(format!("(State {})", id)),
                    ValueView::Memo(h) => result.push(format!("(Memo {} \"{}\")", h.id, h.name)),
                    ValueView::Error(msg, details) => {
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
                        } else if items.first().and_then(|h| h.as_atom()) == Some("Bindings") {
                            let pairs = &items[1..];
                            if pairs.is_empty() {
                                let s = "{ }".to_string();
                                if let Some(k) = memo_key_opt {
                                    memo.insert(k, s.clone());
                                }
                                result.push(s);
                            } else {
                                let mut var_names: Vec<String> = Vec::with_capacity(pairs.len());
                                let mut malformed: Vec<bool> = Vec::with_capacity(pairs.len());
                                for p in pairs {
                                    let (name, ok) = match p.view() {
                                        ValueView::SExpr(kv) if kv.len() == 2 => {
                                            match kv[0].view() {
                                                ValueView::Atom(n) => (n.to_string(), true),
                                                _ => (String::new(), false),
                                            }
                                        }
                                        _ => (String::new(), false),
                                    };
                                    var_names.push(name);
                                    malformed.push(!ok);
                                }
                                work.push(Work::JoinBindings {
                                    var_names,
                                    malformed: malformed.clone(),
                                    memo_key: memo_key_opt,
                                });
                                for (i, p) in pairs.iter().enumerate().rev() {
                                    let to_render = if malformed[i] {
                                        p.clone()
                                    } else if let ValueView::SExpr(kv) = p.view() {
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
                    // PT-canonical Lazy is INVISIBLE — render the inner value
                    // directly. Mirrors the display semantics in main.rs and
                    // the metatype passthrough in models/metta_value.rs.
                    ValueView::Lazy(inner) => {
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
                let formatted = format!("{}{}{}", prefix, parts.join(separator), suffix);
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

// ───────────────────────────────────────────────────────────────────────────
// YAML lite parser helpers
// ───────────────────────────────────────────────────────────────────────────

/// Quote- and paren-aware split of a YAML inline-list body.
///
/// Splits `inner` (the text between `[` and `]`) on top-level commas only:
/// commas inside `"..."` double-quoted strings or inside `(...)` parentheses
/// are preserved.
pub fn split_yaml_list_inner(inner: &str) -> Vec<String> {
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
pub fn strip_yaml_quotes(s: String) -> String {
    let t = s.trim();
    if t.starts_with('"') && t.ends_with('"') && t.len() >= 2 {
        let inner = &t[1..t.len() - 1];
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
        let inner = &t[1..t.len() - 1];
        inner.replace("''", "'")
    } else {
        t.to_string()
    }
}

/// Read the top-level `status:` key from a fixture YAML (if present).
/// Returns the value with quotes stripped.
pub fn read_status(yaml_path: &Path) -> Option<String> {
    let content = fs::read_to_string(yaml_path).ok()?;
    for raw_line in content.lines() {
        let trimmed = raw_line.trim_end();
        let indent = trimmed.len() - trimmed.trim_start().len();
        if indent != 0 {
            continue;
        }
        let body = trimmed.trim_start();
        if let Some(value) = body.strip_prefix("status:") {
            return Some(
                value
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string(),
            );
        }
    }
    None
}

/// Read a top-level scalar string field from YAML (one of the forms
/// `key: value` or `key: "value"`). Returns None if the key isn't present
/// or appears only at non-zero indent.
pub fn read_top_level_scalar(yaml_path: &Path, key: &str) -> Option<String> {
    let content = fs::read_to_string(yaml_path).ok()?;
    let prefix = format!("{}:", key);
    for raw_line in content.lines() {
        let trimmed = raw_line.trim_end();
        let indent = trimmed.len() - trimmed.trim_start().len();
        if indent != 0 {
            continue;
        }
        let body = trimmed.trim_start();
        if let Some(value) = body.strip_prefix(&prefix) {
            return Some(
                value
                    .trim()
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_string(),
            );
        }
    }
    None
}

/// Read a top-level integer field from YAML. Returns None if missing or
/// unparseable.
pub fn read_top_level_int(yaml_path: &Path, key: &str) -> Option<i64> {
    read_top_level_scalar(yaml_path, key).and_then(|s| s.parse::<i64>().ok())
}

/// Read a top-level boolean field from YAML (`true`/`false`/`True`/`False`).
pub fn read_top_level_bool(yaml_path: &Path, key: &str) -> Option<bool> {
    read_top_level_scalar(yaml_path, key).and_then(|s| match s.as_str() {
        "true" | "True" | "yes" => Some(true),
        "false" | "False" | "no" => Some(false),
        _ => None,
    })
}

// ───────────────────────────────────────────────────────────────────────────
// FixtureOutcome — shared result type
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum FixtureOutcome {
    Pass,
    Mismatch {
        expected: Vec<String>,
        actual: Vec<String>,
    },
    /// No expected results in YAML — fixture only asserts non-crash.
    Skipped,
    /// Fixture declares status: host-dependent; pass if shapes match per
    /// `atom_kind_prefix` or any Error atom is present.
    HostDependent,
    /// Fixture declares status: xfail; expected mismatch was confirmed.
    XFailExpected,
    /// Fixture declares xfail but actually matched expected.
    XPassUnexpected {
        expected: Vec<String>,
    },
}

impl FixtureOutcome {
    pub fn counts_as_pass(&self) -> bool {
        matches!(
            self,
            FixtureOutcome::Pass
                | FixtureOutcome::HostDependent
                | FixtureOutcome::XFailExpected
                | FixtureOutcome::XPassUnexpected { .. }
        )
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Argument parsing — shared base options
// ───────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct BaseOptions {
    pub conformance_dir: PathBuf,
    pub module_filter: Option<String>,
    pub strict: bool,
    pub fixture_filter: Option<String>,
}

pub fn parse_base_args(args: &[String]) -> Result<BaseOptions, String> {
    let mut conformance_dir: Option<PathBuf> = None;
    let mut module_filter: Option<String> = None;
    let mut fixture_filter: Option<String> = None;
    let mut strict = false;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
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
            "--fixture" => {
                i += 1;
                if i >= args.len() {
                    return Err("Missing name after --fixture".to_string());
                }
                fixture_filter = Some(args[i].clone());
            }
            "--strict" => {
                strict = true;
            }
            "--tier" => {
                // placeholder for tier filtering — consumed but not yet honored
                i += 1;
            }
            "--help" | "-h" => {
                return Err("__help__".to_string());
            }
            other => return Err(format!("Unknown argument: {}", other)),
        }
        i += 1;
    }

    let conformance_dir =
        conformance_dir.ok_or_else(|| "--conformance-dir is required".to_string())?;
    Ok(BaseOptions {
        conformance_dir,
        module_filter,
        strict,
        fixture_filter,
    })
}
