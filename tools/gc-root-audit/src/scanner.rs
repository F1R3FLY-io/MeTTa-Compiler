//! Pass 1: Scan Rust source files for MettaValue storage locations
//!
//! Uses `syn` to parse Rust files and extract:
//! - Struct/enum definitions with fields containing MettaValue
//! - `impl RootProvider for X` blocks
//! - `static`/`LazyLock`/`OnceLock`/`thread_local!` declarations
//! - Type aliases

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use proc_macro2::Span;
use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::Type;
use walkdir::WalkDir;

/// Result of scanning all source files
#[derive(Debug, Default)]
pub struct ScanResult {
    /// Number of files scanned
    pub files_scanned: usize,

    /// Struct/enum definitions that may contain MettaValue
    pub type_defs: Vec<TypeDef>,

    /// Impl blocks (especially `impl RootProvider for X`)
    pub impl_blocks: Vec<ImplBlock>,

    /// Static/LazyLock/OnceLock/thread_local declarations
    pub statics: Vec<StaticDecl>,

    /// Type aliases
    pub type_aliases: Vec<TypeAlias>,

    /// Set of types that implement RootProvider (from AST: `impl RootProvider for X`)
    pub root_provider_types: HashSet<String>,

    /// Map from type name to generic parameter bounds
    pub generic_bounds: HashMap<String, Vec<GenericBound>>,

    /// Statics that are referenced inside `collect_*_roots` functions.
    /// These are collected directly (not via RootProvider trait) and are safe.
    pub statics_in_root_collectors: HashSet<String>,

    /// Maps RootProvider impl type → statics referenced in its `collect_roots` method.
    /// E.g., "MemoCacheRoots" → {"GLOBAL_MEMO_CACHE"}
    pub statics_in_root_provider_impls: HashMap<String, HashSet<String>>,

    /// RootProvider types with a confirmed `register_root_provider` call.
    /// Populated by scanning free functions and impl method bodies.
    pub verified_registered_providers: HashSet<String>,

    /// Map from type name → set of type names it directly references in fields.
    /// Used for transitive MettaValue reachability analysis.
    /// E.g., "SpaceRegistry" → {"SpaceHandle"}, "SpaceHandle" → {"SpaceBacking", "AtomSpace"}, etc.
    pub type_field_references: HashMap<String, HashSet<String>>,

    /// Map from wrapper function name → static name it wraps.
    /// E.g., "global_tiered_cache" → "GLOBAL_TIERED_CACHE".
    /// Built by scanning function bodies for `& STATIC_NAME` references.
    pub wrapper_fn_to_static: HashMap<String, String>,

    /// Set of direct MettaValue holder types whose impl methods return MettaValue
    /// or a MettaValue-carrying generic param. Types NOT in this set only hold
    /// MettaValue opaquely (e.g., GcSnapshot for mark-sweep pointer extraction).
    pub types_exposing_metta_value: HashSet<String>,

    /// Function-local Vec<MettaValue> variables that are live across
    /// eval_trampoline_generic calls without maybe_push_frame guards.
    pub frame_chain_findings: Vec<FrameChainFinding>,

    /// Analysis of `collect_roots()` method bodies: which `self.field` accesses
    /// and which sub-fields of iterated types are covered.
    /// Maps RootProvider type name → CollectRootsAnalysis.
    pub collect_roots_analyses: HashMap<String, CollectRootsAnalysis>,
}

/// Analysis of a single `collect_roots()` method body.
///
/// Records which fields of `self` are accessed and, for iterator chains
/// over composite types, which sub-fields of the iterated type are accessed.
#[derive(Debug, Clone, Default)]
pub struct CollectRootsAnalysis {
    /// Type name this analysis belongs to
    pub type_name: String,
    /// Fields accessed via `self.<field>` in the method body
    pub covered_fields: HashMap<String, FieldCoverage>,
}

/// How a field is covered in `collect_roots()`.
#[derive(Debug, Clone)]
pub enum FieldCoverage {
    /// Field fully delegated via `.collect_gc_roots(roots)` or similar
    Delegated,
    /// Field accessed directly (e.g., `self.field.read()`)
    /// with specific sub-fields accessed in closures
    Partial(Vec<String>),
    /// Field accessed and all contents collected (e.g., roots.extend(self.field.iter()))
    Full,
}

/// A finding from the frame chain safety checker.
///
/// Indicates a function-local Vec with MettaValue-compatible element type
/// that is live across an `eval_trampoline_generic` call without a
/// `maybe_push_frame` guard — meaning a GC safepoint could free values
/// still referenced by the Vec.
#[derive(Debug, Clone)]
pub struct FrameChainFinding {
    pub file: PathBuf,
    pub line: usize,
    pub function_name: String,
    /// The unguarded Vec local variable name
    pub vec_local: String,
    /// The Vec's type annotation (if available)
    pub vec_type: String,
    /// Line of the eval_trampoline_generic call
    pub trampoline_call_line: usize,
}

/// A struct or enum field that may contain MettaValue
#[derive(Debug, Clone)]
pub struct TypeDef {
    pub file: PathBuf,
    pub line: usize,
    pub name: String,
    pub kind: TypeDefKind,
    pub fields: Vec<FieldInfo>,
    pub type_params: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum TypeDefKind {
    Struct,
    Enum,
}

#[derive(Debug, Clone)]
pub struct FieldInfo {
    pub name: String,
    pub ty: String,
    pub line: usize,
    pub contains_metta_value: bool,
    pub container: Option<String>,
}

/// An impl block
#[derive(Debug, Clone)]
pub struct ImplBlock {
    pub file: PathBuf,
    pub line: usize,
    pub self_ty: String,
    pub trait_name: Option<String>,
    pub has_registration: bool,
}

/// A static, LazyLock, OnceLock, or thread_local declaration
#[derive(Debug, Clone)]
pub struct StaticDecl {
    pub file: PathBuf,
    pub line: usize,
    pub name: String,
    pub ty: String,
    pub kind: StaticKind,
    pub contains_metta_value: bool,
    /// True when the type references MettaValue/MettaValueInner ONLY through
    /// raw pointers (*const/*mut). Raw pointers are not GC roots.
    pub only_raw_ptr_reference: bool,
}

#[derive(Debug, Clone)]
pub enum StaticKind {
    Static,
    LazyLock,
    OnceLock,
    ThreadLocal,
}

impl std::fmt::Display for StaticKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StaticKind::Static => write!(f, "static"),
            StaticKind::LazyLock => write!(f, "LazyLock"),
            StaticKind::OnceLock => write!(f, "OnceLock"),
            StaticKind::ThreadLocal => write!(f, "thread_local!"),
        }
    }
}

/// A type alias
#[derive(Debug, Clone)]
pub struct TypeAlias {
    pub file: PathBuf,
    pub line: usize,
    pub name: String,
    pub target: String,
}

/// A generic type parameter bound
#[derive(Debug, Clone)]
pub struct GenericBound {
    pub param: String,
    pub bound: String,
}

/// Get line number from a Span (requires `span-locations` feature on proc-macro2)
fn span_line(span: Span) -> usize {
    span.start().line
}

/// Convert a syn::Type to a string representation
fn type_to_string(ty: &Type) -> String {
    ty.to_token_stream().to_string()
}

/// Scan a directory for Rust source files and extract MettaValue storage locations
pub fn scan_directory(source_dir: &Path, include_tests: bool) -> ScanResult {
    let mut result = ScanResult::default();

    for entry in WalkDir::new(source_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "rs")
                .unwrap_or(false)
        })
    {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let file = match syn::parse_file(&content) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  Warning: Could not parse {}: {}", path.display(), e);
                continue;
            }
        };

        result.files_scanned += 1;

        // Visitor A: Collect MettaValue storage locations (existing)
        let mut visitor = MettaValueVisitor {
            file_path: path.to_path_buf(),
            include_tests,
            result: &mut result,
            in_test_module: false,
        };
        visitor.visit_file(&file);

        // Visitor B: Collect field type references for ALL types (for transitive reachability)
        let mut type_ref_visitor = TypeFieldRefVisitor {
            result: &mut result.type_field_references,
        };
        type_ref_visitor.visit_file(&file);
    }

    // Identify which types implement RootProvider (from AST)
    for imp in &result.impl_blocks {
        if imp.trait_name.as_deref() == Some("RootProvider") {
            result.root_provider_types.insert(imp.self_ty.clone());
        }
    }

    // Second pass: find root collection functions, RootProvider impl bodies,
    // verified register_root_provider calls, wrapper functions, and data access patterns.
    let static_names: Vec<String> = result.statics.iter().map(|s| s.name.clone()).collect();
    let root_provider_types = result.root_provider_types.clone();
    let mut statics_in_rp_impls: HashMap<String, HashSet<String>> = HashMap::new();
    let mut verified_providers: HashSet<String> = HashSet::new();
    let mut wrapper_fn_to_static: HashMap<String, String> = HashMap::new();

    // Build direct_holders map for DataAccessVisitor: type_name → type_params
    // for every type_def that directly holds MettaValue in a field.
    let direct_holders: HashMap<String, Vec<String>> = result
        .type_defs
        .iter()
        .filter(|td| td.fields.iter().any(|f| f.contains_metta_value))
        .map(|td| (td.name.clone(), td.type_params.clone()))
        .collect();
    let mut types_exposing_metta_value: HashSet<String> = HashSet::new();

    for entry in WalkDir::new(source_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "rs")
                .unwrap_or(false)
        })
    {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let file = match syn::parse_file(&content) {
            Ok(f) => f,
            Err(_) => continue,
        };

        // Visitor 1: Free functions named collect_*_roots
        let mut fn_visitor = RootCollectorFnVisitor {
            static_names: &static_names,
            found: &mut result.statics_in_root_collectors,
        };
        fn_visitor.visit_file(&file);

        // Visitor 2: impl RootProvider blocks — which statics does collect_roots reference?
        let mut rp_impl_visitor = RootProviderImplVisitor {
            static_names: &static_names,
            wrapper_fn_to_static: &wrapper_fn_to_static,
            found: &mut statics_in_rp_impls,
        };
        rp_impl_visitor.visit_file(&file);

        // Visitor 3: Verified register_root_provider calls — which provider types are registered?
        let mut verified_visitor = VerifiedRegistrationVisitor {
            root_provider_types: &root_provider_types,
            verified: &mut verified_providers,
        };
        verified_visitor.visit_file(&file);

        // Visitor 4: Wrapper functions (e.g., global_tiered_cache → GLOBAL_TIERED_CACHE)
        let mut wrapper_visitor = WrapperFnVisitor {
            static_names: &static_names,
            found: &mut wrapper_fn_to_static,
        };
        wrapper_visitor.visit_file(&file);

        // Visitor 5: Data access patterns — which types expose MettaValue via return types?
        let mut data_access_visitor = DataAccessVisitor {
            direct_holders: &direct_holders,
            exposing: &mut types_exposing_metta_value,
        };
        data_access_visitor.visit_file(&file);
    }

    result.statics_in_root_provider_impls = statics_in_rp_impls;
    result.verified_registered_providers = verified_providers;
    result.wrapper_fn_to_static = wrapper_fn_to_static;
    result.types_exposing_metta_value = types_exposing_metta_value;

    // Third pass: Re-run RootProvider impl scanning now that wrapper_fn_to_static
    // is fully populated (the first pass may have processed RootProvider impls
    // before the wrapper function definitions were scanned).
    let mut statics_in_rp_impls_final: HashMap<String, HashSet<String>> = HashMap::new();
    for entry in WalkDir::new(source_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "rs")
                .unwrap_or(false)
        })
    {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let file = match syn::parse_file(&content) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let mut rp_impl_visitor = RootProviderImplVisitor {
            static_names: &static_names,
            wrapper_fn_to_static: &result.wrapper_fn_to_static,
            found: &mut statics_in_rp_impls_final,
        };
        rp_impl_visitor.visit_file(&file);
    }
    result.statics_in_root_provider_impls = statics_in_rp_impls_final;

    // Fourth pass: Frame chain safety checker — find Vec locals live across
    // eval_trampoline_generic calls without maybe_push_frame guards.
    let mut frame_chain_findings = Vec::new();
    for entry in WalkDir::new(source_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "rs")
                .unwrap_or(false)
        })
    {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let file = match syn::parse_file(&content) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let mut fc_visitor = FrameChainVisitor {
            file_path: path.to_path_buf(),
            findings: &mut frame_chain_findings,
        };
        fc_visitor.visit_file(&file);
    }
    result.frame_chain_findings = frame_chain_findings;

    // Fifth pass: collect_roots() field coverage analysis — determine which
    // self.field accesses and sub-field accesses occur in each RootProvider's
    // collect_roots() method body. Used by classifier Phase 8 to detect
    // MettaValue-bearing fields not covered by GC root collection.
    let mut collect_roots_analyses = HashMap::new();
    for entry in WalkDir::new(source_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "rs")
                .unwrap_or(false)
        })
    {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let file = match syn::parse_file(&content) {
            Ok(f) => f,
            Err(_) => continue,
        };

        let mut cr_visitor = CollectRootsFieldVisitor {
            analyses: &mut collect_roots_analyses,
        };
        cr_visitor.visit_file(&file);
    }
    result.collect_roots_analyses = collect_roots_analyses;

    result
}

/// Second-pass visitor that finds free functions named `collect_*_roots`
/// and detects which statics they reference.
struct RootCollectorFnVisitor<'a> {
    static_names: &'a [String],
    found: &'a mut HashSet<String>,
}

impl<'a, 'ast> Visit<'ast> for RootCollectorFnVisitor<'a> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let fn_name = node.sig.ident.to_string();

        // Detect root collection functions
        if fn_name.contains("collect") && fn_name.contains("root") {
            let body_str = node.block.to_token_stream().to_string();
            for name in self.static_names {
                if body_str.contains(name.as_str()) {
                    self.found.insert(name.clone());
                }
            }
        }

        syn::visit::visit_item_fn(self, node);
    }
}

/// Visitor that finds `impl RootProvider for X` blocks and records which statics
/// are referenced in the `collect_roots` method body.
///
/// Also resolves wrapper function calls: if `collect_roots` calls
/// `global_tiered_cache()` and `wrapper_fn_to_static` maps that to
/// `GLOBAL_TIERED_CACHE`, the static is marked as covered.
struct RootProviderImplVisitor<'a> {
    static_names: &'a [String],
    /// Map from wrapper function name → underlying static name
    wrapper_fn_to_static: &'a HashMap<String, String>,
    /// Output: maps RootProvider type → statics referenced in collect_roots
    found: &'a mut HashMap<String, HashSet<String>>,
}

impl<'a, 'ast> Visit<'ast> for RootProviderImplVisitor<'a> {
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        // Only interested in `impl RootProvider for X`
        let trait_name = node.trait_.as_ref().map(|(_, path, _)| {
            path.segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        });

        if trait_name.as_deref() == Some("RootProvider") {
            let self_ty = type_to_string(&node.self_ty);
            let base_ty = self_ty
                .split('<')
                .next()
                .unwrap_or(&self_ty)
                .trim()
                .split("::")
                .last()
                .unwrap_or(&self_ty)
                .to_string();

            // Find the `collect_roots` method and search its body for static names
            for item in &node.items {
                if let syn::ImplItem::Fn(method) = item {
                    if method.sig.ident == "collect_roots" {
                        let body_str = method.block.to_token_stream().to_string();

                        // Direct static name references
                        for name in self.static_names {
                            if body_str.contains(name.as_str()) {
                                self.found
                                    .entry(base_ty.clone())
                                    .or_default()
                                    .insert(name.clone());
                            }
                        }

                        // Resolve wrapper function calls to their underlying statics.
                        // E.g., if body calls `global_tiered_cache()` and wrapper map has
                        // "global_tiered_cache" → "GLOBAL_TIERED_CACHE", mark it covered.
                        for (fn_name, static_name) in self.wrapper_fn_to_static {
                            if body_str.contains(fn_name.as_str()) {
                                self.found
                                    .entry(base_ty.clone())
                                    .or_default()
                                    .insert(static_name.clone());
                            }
                        }
                    }
                }
            }
        }

        syn::visit::visit_item_impl(self, node);
    }
}

/// Visitor that scans all functions (free and impl methods) for `register_root_provider`
/// calls and records which RootProvider types appear in the same function body.
struct VerifiedRegistrationVisitor<'a> {
    root_provider_types: &'a HashSet<String>,
    /// Output: RootProvider types confirmed to have register_root_provider call
    verified: &'a mut HashSet<String>,
}

impl<'a, 'ast> Visit<'ast> for VerifiedRegistrationVisitor<'a> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let body_str = node.block.to_token_stream().to_string();
        self.check_body_for_registration(&body_str);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        // Check each method in the impl block
        for item in &node.items {
            if let syn::ImplItem::Fn(method) = item {
                let body_str = method.block.to_token_stream().to_string();
                if body_str.contains("register_root_provider") {
                    // Check which root_provider_types appear in this method body
                    for rp_type in self.root_provider_types {
                        let base = rp_type
                            .split('<')
                            .next()
                            .unwrap_or(rp_type)
                            .trim()
                            .split("::")
                            .last()
                            .unwrap_or(rp_type);
                        if body_str.contains(base) {
                            self.verified.insert(base.to_string());
                        }
                    }

                    // Also check for the cross-type pattern: if the impl's self_ty
                    // has register_root_provider and references a root provider type
                    // (e.g., MettaState::new() registering MettaStateGcRoots)
                    let self_ty = type_to_string(&node.self_ty);
                    let self_base = self_ty
                        .split('<')
                        .next()
                        .unwrap_or(&self_ty)
                        .trim()
                        .split("::")
                        .last()
                        .unwrap_or(&self_ty)
                        .to_string();

                    for rp_type in self.root_provider_types {
                        let base = rp_type
                            .split('<')
                            .next()
                            .unwrap_or(rp_type)
                            .trim()
                            .split("::")
                            .last()
                            .unwrap_or(rp_type);
                        // Check if the root provider type name appears in
                        // the method body (e.g., `MettaStateGcRoots` in `MettaState::new`)
                        if body_str.contains(base) && base != self_base {
                            self.verified.insert(base.to_string());
                        }
                    }
                }
            }
        }

        syn::visit::visit_item_impl(self, node);
    }
}

impl<'a> VerifiedRegistrationVisitor<'a> {
    fn check_body_for_registration(&mut self, body_str: &str) {
        if !body_str.contains("register_root_provider") {
            return;
        }

        for rp_type in self.root_provider_types {
            let base = rp_type
                .split('<')
                .next()
                .unwrap_or(rp_type)
                .trim()
                .split("::")
                .last()
                .unwrap_or(rp_type);
            if body_str.contains(base) {
                self.verified.insert(base.to_string());
            }
        }
    }
}

/// Check if a type string references MettaValue or MettaValueInner
fn type_references_metta_value(ty: &str) -> bool {
    ty.contains("MettaValue")
        || ty.contains("MettaValueInner")
        || ty.contains("MettaValueTrait")
}

/// Check if a type string references MettaValue/MettaValueInner ONLY through
/// raw pointers (`*const` or `*mut`). In proc_macro2 tokenized output, `*const`
/// becomes `* const` (with space). Returns false if any occurrence is NOT
/// preceded by a raw pointer prefix, or if there are no occurrences.
fn type_references_metta_value_only_via_raw_ptr(ty: &str) -> bool {
    let keywords = ["MettaValueInner", "MettaValueTrait", "MettaValue"];
    let mut positions: Vec<(usize, usize)> = Vec::new();

    for keyword in &keywords {
        let mut search_from = 0;
        while let Some(pos) = ty[search_from..].find(keyword) {
            let abs_start = search_from + pos;
            let abs_end = abs_start + keyword.len();
            // Skip if overlapping with already-found longer match
            if !positions.iter().any(|&(s, e)| abs_start >= s && abs_start < e) {
                positions.push((abs_start, abs_end));
            }
            search_from = abs_start + 1;
        }
    }

    if positions.is_empty() {
        return false;
    }

    // Every occurrence must be preceded by "* const" or "* mut"
    positions.iter().all(|&(start, _)| {
        let prefix = ty[..start].trim_end();
        prefix.ends_with("* const") || prefix.ends_with("* mut")
    })
}

/// Check if a type string references a generic V that might be MettaValue.
/// Uses word-boundary matching to avoid matching substrings like "V" in "Vec".
fn type_references_generic_value(ty: &str, generic_params: &[String]) -> bool {
    for param in generic_params {
        if param.len() == 1 {
            // Single-char params (V, E, F, C, T): use word-boundary matching
            let param_char = param.chars().next().expect("param should be non-empty");
            let chars: Vec<char> = ty.chars().collect();
            for (i, &ch) in chars.iter().enumerate() {
                if ch == param_char {
                    let before_ok = i == 0 || !chars[i - 1].is_alphanumeric();
                    let after_ok = i + 1 >= chars.len() || !chars[i + 1].is_alphanumeric();
                    if before_ok && after_ok {
                        return true;
                    }
                }
            }
        } else {
            // Multi-char params: simple contains is sufficient
            if ty.contains(param.as_str()) {
                return true;
            }
        }
    }
    false
}

/// Extract the outermost container type
fn extract_container(ty: &str) -> Option<String> {
    let containers = [
        "Arc",
        "Vec",
        "DashMap",
        "HashMap",
        "BTreeMap",
        "LruCache",
        "Mutex",
        "RwLock",
        "OnceLock",
        "LazyLock",
        "Option",
        "Box",
    ];
    for c in &containers {
        if ty.starts_with(c) || ty.contains(&format!("{} <", c)) || ty.contains(&format!("{}<", c))
        {
            return Some(c.to_string());
        }
    }
    None
}

/// Extract base type names referenced in a type string.
///
/// Strips common containers and primitive types, returning the remaining
/// PascalCase identifiers. Used to build the type field reference graph
/// for transitive MettaValue reachability analysis.
///
/// E.g., `"Arc<AtomSpace<MettaValue>>"` → `{"AtomSpace", "MettaValue"}`
///       `"DashMap<String, SpaceHandle>"` → `{"SpaceHandle"}`
///       `"Vec<u64>"` → `{}`
pub fn extract_referenced_type_names(ty: &str) -> HashSet<String> {
    const CONTAINERS: &[&str] = &[
        "Arc",
        "Vec",
        "Box",
        "Option",
        "Mutex",
        "RwLock",
        "OnceLock",
        "LazyLock",
        "DashMap",
        "HashMap",
        "BTreeMap",
        "LruCache",
        "SmallVec",
        "VecDeque",
        "HashSet",
        "BTreeSet",
        "Weak",
        "RefCell",
        "Cell",
        "AtomicPtr",
        "Sender",
        "Receiver",
        "Condvar",
    ];
    const PRIMITIVES: &[&str] = &[
        "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128", "isize",
        "f32", "f64", "bool", "String", "str", "char",
    ];

    let mut result = HashSet::new();
    // Split on non-alphanumeric/underscore chars to get tokens
    for token in ty.split(|c: char| !c.is_alphanumeric() && c != '_') {
        let t = token.trim();
        if t.is_empty() || t.len() < 2 {
            continue;
        }
        if CONTAINERS.contains(&t) || PRIMITIVES.contains(&t) {
            continue;
        }
        // Keep PascalCase identifiers (start with uppercase)
        if t.chars()
            .next()
            .map(|c| c.is_uppercase())
            .unwrap_or(false)
        {
            result.insert(t.to_string());
        }
    }
    result
}

/// Collect referenced type names from `syn::Fields` (named, unnamed, or unit).
fn extract_field_type_refs(fields: &syn::Fields) -> HashSet<String> {
    let mut refs = HashSet::new();
    match fields {
        syn::Fields::Named(named) => {
            for field in &named.named {
                let ty_str = type_to_string(&field.ty);
                refs.extend(extract_referenced_type_names(&ty_str));
            }
        }
        syn::Fields::Unnamed(unnamed) => {
            for field in &unnamed.unnamed {
                let ty_str = type_to_string(&field.ty);
                refs.extend(extract_referenced_type_names(&ty_str));
            }
        }
        syn::Fields::Unit => {}
    }
    refs
}

/// Lightweight visitor that records field type references for ALL struct/enum
/// definitions — not just those containing MettaValue. This builds the type
/// graph needed for transitive MettaValue reachability analysis.
struct TypeFieldRefVisitor<'a> {
    result: &'a mut HashMap<String, HashSet<String>>,
}

impl<'a, 'ast> Visit<'ast> for TypeFieldRefVisitor<'a> {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        let name = node.ident.to_string();
        let refs = extract_field_type_refs(&node.fields);
        if !refs.is_empty() {
            self.result.entry(name).or_default().extend(refs);
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        let name = node.ident.to_string();
        for variant in &node.variants {
            let refs = extract_field_type_refs(&variant.fields);
            self.result.entry(name.clone()).or_default().extend(refs);
        }
        syn::visit::visit_item_enum(self, node);
    }
}

/// Visitor that detects wrapper functions that return a reference to a known static.
///
/// Pattern: `fn global_tiered_cache() -> &'static ... { &GLOBAL_TIERED_CACHE }`
/// syn tokenizes `&GLOBAL_FOO` as `& GLOBAL_FOO` in the token stream.
struct WrapperFnVisitor<'a> {
    static_names: &'a [String],
    /// Output: maps wrapper function name → static name it wraps
    found: &'a mut HashMap<String, String>,
}

impl<'a, 'ast> Visit<'ast> for WrapperFnVisitor<'a> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        let fn_name = node.sig.ident.to_string();
        let body_str = node.block.to_token_stream().to_string();
        for name in self.static_names {
            // syn tokenizes `&GLOBAL_FOO` as `& GLOBAL_FOO`
            if body_str.contains(&format!("& {}", name))
                || body_str.contains(&format!("&{}", name))
            {
                self.found.insert(fn_name.clone(), name.clone());
                break;
            }
        }
        syn::visit::visit_item_fn(self, node);
    }
}

/// Visitor that checks whether direct MettaValue holder types expose MettaValue
/// through their method return types.
///
/// A type "exposes" MettaValue if any method in its `impl` block returns a type
/// that contains MettaValue or a MettaValue-carrying generic param. Types that
/// never return MettaValue are "opaque" — they hold MettaValue for internal
/// bookkeeping (e.g., GC mark-sweep) but external code can't dereference it.
struct DataAccessVisitor<'a> {
    /// Direct MettaValue holder type names → their type params
    direct_holders: &'a HashMap<String, Vec<String>>,
    /// Output: types whose methods return MettaValue/carrier params
    exposing: &'a mut HashSet<String>,
}

impl<'a, 'ast> Visit<'ast> for DataAccessVisitor<'a> {
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        let self_ty = type_to_string(&node.self_ty);
        // Check if this impl is for a direct holder type
        for (type_name, type_params) in self.direct_holders {
            if !self_ty.contains(type_name.as_str()) {
                continue;
            }

            // Check each method's return type
            for item in &node.items {
                if let syn::ImplItem::Fn(method) = item {
                    if let syn::ReturnType::Type(_, ret_ty) = &method.sig.output {
                        let ret_str = type_to_string(ret_ty);
                        // Check if return type contains MettaValue or a carrier param
                        if type_references_metta_value(&ret_str)
                            || type_references_generic_value(&ret_str, type_params)
                        {
                            self.exposing.insert(type_name.clone());
                        }
                    }
                }
            }
            break;
        }
        syn::visit::visit_item_impl(self, node);
    }
}

/// Extract generic bounds from both inline type params and `where` clauses.
///
/// Covers two patterns:
/// 1. Inline: `struct Foo<V: MettaValueTrait>` → param "V", bound "MettaValueTrait"
/// 2. Where clause: `struct Foo<V> where V: MettaValueTrait` → same result
fn collect_generic_bounds(generics: &syn::Generics) -> Vec<GenericBound> {
    let mut bounds = Vec::new();

    // 1. Inline bounds: `<V: MettaValueTrait + Clone>`
    for tp in generics.type_params() {
        for bound in &tp.bounds {
            if let syn::TypeParamBound::Trait(tb) = bound {
                let bound_name = tb
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::");
                bounds.push(GenericBound {
                    param: tp.ident.to_string(),
                    bound: bound_name,
                });
            }
        }
    }

    // 2. Where clause bounds: `where V: MettaValueTrait + Clone`
    if let Some(where_clause) = &generics.where_clause {
        for predicate in &where_clause.predicates {
            if let syn::WherePredicate::Type(type_pred) = predicate {
                // Extract the bounded type — must be a simple ident (type param name)
                let param_name = type_to_string(&type_pred.bounded_ty);
                let param_name = param_name.trim().to_string();

                for bound in &type_pred.bounds {
                    if let syn::TypeParamBound::Trait(tb) = bound {
                        let bound_name = tb
                            .path
                            .segments
                            .iter()
                            .map(|s| s.ident.to_string())
                            .collect::<Vec<_>>()
                            .join("::");
                        bounds.push(GenericBound {
                            param: param_name.clone(),
                            bound: bound_name,
                        });
                    }
                }
            }
        }
    }

    bounds
}

/// AST visitor that collects MettaValue storage locations
struct MettaValueVisitor<'a> {
    file_path: PathBuf,
    include_tests: bool,
    result: &'a mut ScanResult,
    in_test_module: bool,
}

impl<'a, 'ast> Visit<'ast> for MettaValueVisitor<'a> {
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if self.in_test_module && !self.include_tests {
            return;
        }

        let name = node.ident.to_string();
        let type_params: Vec<String> = node
            .generics
            .type_params()
            .map(|tp| tp.ident.to_string())
            .collect();

        // Collect generic bounds (inline + where clause)
        let generic_bounds = collect_generic_bounds(&node.generics);
        if !generic_bounds.is_empty() {
            self.result
                .generic_bounds
                .entry(name.clone())
                .or_default()
                .extend(generic_bounds);
        }

        let mut fields = Vec::new();
        let mut has_metta_value = false;

        match &node.fields {
            syn::Fields::Named(named) => {
                for field in &named.named {
                    let field_name = field
                        .ident
                        .as_ref()
                        .map(|i| i.to_string())
                        .unwrap_or_default();
                    let ty_str = type_to_string(&field.ty);
                    let contains = type_references_metta_value(&ty_str)
                        || type_references_generic_value(&ty_str, &type_params);
                    let container = extract_container(&ty_str);

                    if contains {
                        has_metta_value = true;
                    }

                    fields.push(FieldInfo {
                        name: field_name,
                        ty: ty_str,
                        line: field
                            .ident
                            .as_ref()
                            .map(|i| span_line(i.span()))
                            .unwrap_or(0),
                        contains_metta_value: contains,
                        container,
                    });
                }
            }
            syn::Fields::Unnamed(unnamed) => {
                for (i, field) in unnamed.unnamed.iter().enumerate() {
                    let ty_str = type_to_string(&field.ty);
                    let contains = type_references_metta_value(&ty_str)
                        || type_references_generic_value(&ty_str, &type_params);
                    let container = extract_container(&ty_str);

                    if contains {
                        has_metta_value = true;
                    }

                    fields.push(FieldInfo {
                        name: format!("{}", i),
                        ty: ty_str,
                        line: 0,
                        contains_metta_value: contains,
                        container,
                    });
                }
            }
            syn::Fields::Unit => {}
        }

        if has_metta_value {
            self.result.type_defs.push(TypeDef {
                file: self.file_path.clone(),
                line: span_line(node.ident.span()),
                name: name.clone(),
                kind: TypeDefKind::Struct,
                fields,
                type_params,
            });
        }

        syn::visit::visit_item_struct(self, node);
    }

    fn visit_item_enum(&mut self, node: &'ast syn::ItemEnum) {
        if self.in_test_module && !self.include_tests {
            return;
        }

        let name = node.ident.to_string();
        let type_params: Vec<String> = node
            .generics
            .type_params()
            .map(|tp| tp.ident.to_string())
            .collect();

        // Collect generic bounds (inline + where clause)
        let generic_bounds = collect_generic_bounds(&node.generics);
        if !generic_bounds.is_empty() {
            self.result
                .generic_bounds
                .entry(name.clone())
                .or_default()
                .extend(generic_bounds);
        }

        let mut fields = Vec::new();
        let mut has_metta_value = false;

        for variant in &node.variants {
            let variant_name = variant.ident.to_string();
            match &variant.fields {
                syn::Fields::Named(named) => {
                    for field in &named.named {
                        let field_name = field
                            .ident
                            .as_ref()
                            .map(|i| i.to_string())
                            .unwrap_or_default();
                        let ty_str = type_to_string(&field.ty);
                        let contains = type_references_metta_value(&ty_str)
                            || type_references_generic_value(&ty_str, &type_params);
                        let container = extract_container(&ty_str);

                        if contains {
                            has_metta_value = true;
                        }

                        fields.push(FieldInfo {
                            name: format!("{}::{}", variant_name, field_name),
                            ty: ty_str,
                            line: span_line(variant.ident.span()),
                            contains_metta_value: contains,
                            container,
                        });
                    }
                }
                syn::Fields::Unnamed(unnamed) => {
                    for (i, field) in unnamed.unnamed.iter().enumerate() {
                        let ty_str = type_to_string(&field.ty);
                        let contains = type_references_metta_value(&ty_str)
                            || type_references_generic_value(&ty_str, &type_params);
                        let container = extract_container(&ty_str);

                        if contains {
                            has_metta_value = true;
                        }

                        fields.push(FieldInfo {
                            name: format!("{}::{}", variant_name, i),
                            ty: ty_str,
                            line: span_line(variant.ident.span()),
                            contains_metta_value: contains,
                            container,
                        });
                    }
                }
                syn::Fields::Unit => {}
            }
        }

        if has_metta_value {
            self.result.type_defs.push(TypeDef {
                file: self.file_path.clone(),
                line: span_line(node.ident.span()),
                name: name.clone(),
                kind: TypeDefKind::Enum,
                fields,
                type_params,
            });
        }

        syn::visit::visit_item_enum(self, node);
    }

    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if self.in_test_module && !self.include_tests {
            return;
        }

        let self_ty = type_to_string(&node.self_ty);
        let trait_name = node.trait_.as_ref().map(|(_, path, _)| {
            path.segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        });

        // Check if any method body contains register_root_provider
        let has_registration = {
            let impl_str = node.to_token_stream().to_string();
            impl_str.contains("register_root_provider")
        };

        self.result.impl_blocks.push(ImplBlock {
            file: self.file_path.clone(),
            line: span_line(node.self_ty.span()),
            self_ty,
            trait_name,
            has_registration,
        });

        syn::visit::visit_item_impl(self, node);
    }

    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        if self.in_test_module && !self.include_tests {
            return;
        }

        let name = node.ident.to_string();
        let ty_str = type_to_string(&node.ty);
        let contains = type_references_metta_value(&ty_str);

        let kind = if ty_str.contains("LazyLock") {
            StaticKind::LazyLock
        } else if ty_str.contains("OnceLock") {
            StaticKind::OnceLock
        } else {
            StaticKind::Static
        };

        self.result.statics.push(StaticDecl {
            file: self.file_path.clone(),
            line: span_line(node.ident.span()),
            name,
            ty: ty_str.clone(),
            kind,
            contains_metta_value: contains,
            only_raw_ptr_reference: contains && type_references_metta_value_only_via_raw_ptr(&ty_str),
        });

        syn::visit::visit_item_static(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast syn::ItemType) {
        if self.in_test_module && !self.include_tests {
            return;
        }

        let name = node.ident.to_string();
        let target = type_to_string(&node.ty);

        if type_references_metta_value(&target) {
            self.result.type_aliases.push(TypeAlias {
                file: self.file_path.clone(),
                line: span_line(node.ident.span()),
                name,
                target,
            });
        }

        syn::visit::visit_item_type(self, node);
    }

    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        // Detect test modules
        let is_test = node.attrs.iter().any(|attr| {
            let path_str = attr
                .path()
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::");
            path_str == "cfg" && attr.to_token_stream().to_string().contains("test")
        });

        let prev = self.in_test_module;
        if is_test {
            self.in_test_module = true;
        }

        syn::visit::visit_item_mod(self, node);

        self.in_test_module = prev;
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        // Detect thread_local! macros
        let path_str = node
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::");

        if path_str == "thread_local" {
            let tokens = node.tokens.to_string();
            if type_references_metta_value(&tokens) {
                let name = tokens
                    .split_whitespace()
                    .skip_while(|w| *w != "static")
                    .nth(1)
                    .unwrap_or("UNKNOWN")
                    .trim_end_matches(':')
                    .to_string();

                self.result.statics.push(StaticDecl {
                    file: self.file_path.clone(),
                    line: span_line(node.path.segments.first().map(|s| s.ident.span()).unwrap_or_else(Span::call_site)),
                    name,
                    ty: tokens.clone(),
                    kind: StaticKind::ThreadLocal,
                    contains_metta_value: true,
                    only_raw_ptr_reference: type_references_metta_value_only_via_raw_ptr(&tokens),
                });
            }
        }

        syn::visit::visit_macro(self, node);
    }
}

// =========================================================================
// collect_roots() Field Coverage Analyzer
//
// For each `impl RootProvider for X`, analyzes the `collect_roots()` method
// body to determine which `self.field` accesses occur and, for iterator
// chains like `.flat_map(|e| [e.lhs, e.rhs])`, which sub-fields of the
// iterated type are accessed. This allows the classifier to detect
// MettaValue-bearing fields not covered by root collection — the exact
// class of bug that caused the PLN regression (rhs_type, inferred_fn_types).
// =========================================================================

/// Visitor that finds `impl RootProvider for X` blocks and analyzes
/// the `collect_roots` method body for field coverage.
struct CollectRootsFieldVisitor<'a> {
    analyses: &'a mut HashMap<String, CollectRootsAnalysis>,
}

impl<'a, 'ast> Visit<'ast> for CollectRootsFieldVisitor<'a> {
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        // Only interested in `impl RootProvider for X`
        let trait_name = node.trait_.as_ref().map(|(_, path, _)| {
            path.segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect::<Vec<_>>()
                .join("::")
        });

        if trait_name.as_deref() == Some("RootProvider") {
            let self_ty = type_to_string(&node.self_ty);
            let base_ty = self_ty
                .split('<')
                .next()
                .unwrap_or(&self_ty)
                .trim()
                .split("::")
                .last()
                .unwrap_or(&self_ty)
                .to_string();

            // Find the `collect_roots` method and analyze its body
            for item in &node.items {
                if let syn::ImplItem::Fn(method) = item {
                    if method.sig.ident == "collect_roots" {
                        let analysis = analyze_collect_roots_body(&base_ty, &method.block);
                        self.analyses.insert(base_ty.clone(), analysis);
                    }
                }
            }
        }

        syn::visit::visit_item_impl(self, node);
    }
}

/// Analyze a `collect_roots()` method body to determine field coverage.
///
/// Extracts:
/// 1. `self.<field>` accesses (direct field references)
/// 2. For delegation patterns like `self.field.collect_gc_roots(roots)`, mark as Delegated
/// 3. For iterator chains like `.flat_map(|e| [e.lhs, e.rhs])`, extract sub-field names
/// 4. For direct iteration like `self.field.iter()` → Full coverage
fn analyze_collect_roots_body(type_name: &str, block: &syn::Block) -> CollectRootsAnalysis {
    let body_str = block.to_token_stream().to_string();
    let mut analysis = CollectRootsAnalysis {
        type_name: type_name.to_string(),
        covered_fields: HashMap::new(),
    };

    // Strategy: tokenize the body string and look for patterns.
    // This is more reliable than walking the AST for complex method chains.

    // Pattern 1: self . <field> . collect_gc_roots ( roots )
    // → Delegated coverage
    for field_name in extract_self_field_names(&body_str) {
        // Check if this field is delegated
        let delegation_pattern = format!("self . {} . collect_gc_roots", field_name);
        let delegation_pattern2 = format!("self . {} . collect_gc_roots", field_name);
        if body_str.contains(&delegation_pattern) || body_str.contains(&delegation_pattern2) {
            analysis
                .covered_fields
                .insert(field_name, FieldCoverage::Delegated);
            continue;
        }

        // Pattern 2: Check for iterator chains with sub-field access
        // e.g., rule_index . read () ... .flat_map(|e| [e.lhs, e.rhs])
        // or: self . <field> . iter () ... roots.extend(...)
        let sub_fields = extract_iterator_sub_fields(&body_str, &field_name);
        if !sub_fields.is_empty() {
            analysis
                .covered_fields
                .insert(field_name, FieldCoverage::Partial(sub_fields));
        } else {
            // Field is accessed but we can't determine sub-field coverage,
            // assume full coverage for simple patterns like `self.field.iter()`
            // or `self.field.read()` followed by direct iteration
            analysis
                .covered_fields
                .insert(field_name, FieldCoverage::Full);
        }
    }

    analysis
}

/// Extract all field names accessed via `self . <field>` in a token stream string.
fn extract_self_field_names(body_str: &str) -> Vec<String> {
    let mut fields = Vec::new();
    // syn tokenizes `self.field` as `self . field`
    let pattern = "self . ";
    let mut search_from = 0;
    while let Some(pos) = body_str[search_from..].find(pattern) {
        let abs_pos = search_from + pos + pattern.len();
        // Extract the identifier that follows
        let rest = &body_str[abs_pos..];
        let field_name: String = rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !field_name.is_empty()
            && field_name != "collect_roots" // skip method name itself
            && field_name != "len"
            && !fields.contains(&field_name)
        {
            fields.push(field_name);
        }
        search_from = abs_pos;
    }
    fields
}

/// For a field accessed via iterator chain, extract which sub-fields of the
/// iterated element are accessed in closures.
///
/// Looks for patterns like:
/// - `.flat_map ( | e | [ e . lhs , e . rhs ] )` → ["lhs", "rhs"]
/// - `.flat_map ( | e | { ... e . lhs ... e . rhs ... } )` → ["lhs", "rhs"]
/// - `.map ( | e | e . value )` → ["value"]
///
/// Returns empty vec if no sub-field access pattern is found (indicating
/// the field contents are collected in full, or the pattern is unrecognized).
fn extract_iterator_sub_fields(body_str: &str, field_name: &str) -> Vec<String> {
    // Look for the field name followed by a chain that includes flat_map or map
    // with a closure that accesses sub-fields via `<param> . <sub_field>`
    let field_region = find_field_chain_region(body_str, field_name);
    let region = match field_region {
        Some(r) => r,
        None => return Vec::new(),
    };

    // Look for closure patterns: | <param> | followed by <param> . <sub_field>
    let mut sub_fields = Vec::new();

    // Find closure parameters: `| e |` or `| entry |`
    let closure_params = extract_closure_params(&region);
    for param in &closure_params {
        // Find all `<param> . <sub_field>` patterns in the region
        let param_pattern = format!("{} . ", param);
        let mut search_from = 0;
        while let Some(pos) = region[search_from..].find(&param_pattern) {
            let abs_pos = search_from + pos + param_pattern.len();
            let rest = &region[abs_pos..];
            let sub_field: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !sub_field.is_empty()
                && sub_field != "clone"
                && sub_field != "iter"
                && sub_field != "into_iter"
                && sub_field != "len"
                && !sub_fields.contains(&sub_field)
            {
                sub_fields.push(sub_field);
            }
            search_from = abs_pos;
        }
    }

    sub_fields
}

/// Find the region of the body string that corresponds to a field's iterator chain.
///
/// Starting from `self . <field>`, captures everything up to the next semicolon
/// or the end of the enclosing block.
fn find_field_chain_region(body_str: &str, field_name: &str) -> Option<String> {
    let pattern = format!("self . {}", field_name);
    let pos = body_str.find(&pattern)?;
    let rest = &body_str[pos..];

    // Find the end of the statement (next semicolon at depth 0)
    let mut depth = 0;
    let mut end = rest.len();
    for (i, ch) in rest.char_indices() {
        match ch {
            '{' | '[' | '(' => depth += 1,
            '}' | ']' | ')' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            ';' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }

    Some(rest[..end].to_string())
}

/// Extract closure parameter names from a code region.
///
/// Finds `| <param> |` patterns (syn tokenizes closures with spaces around pipes).
fn extract_closure_params(region: &str) -> Vec<String> {
    let mut params = Vec::new();
    // Look for `| <ident> |` pattern
    let chars: Vec<char> = region.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '|' {
            // Skip whitespace
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            // Collect identifier
            let mut ident = String::new();
            while j < chars.len() && (chars[j].is_alphanumeric() || chars[j] == '_') {
                ident.push(chars[j]);
                j += 1;
            }
            // Skip whitespace
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            // Check for closing pipe
            if j < chars.len() && chars[j] == '|' && !ident.is_empty() {
                if !params.contains(&ident) {
                    params.push(ident);
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    params
}

// =========================================================================
// Frame Chain Safety Checker
//
// Detects function-local Vec<MettaValue> variables that are live across
// eval_trampoline_generic calls without maybe_push_frame guards.
// =========================================================================

/// Top-level visitor dispatching into free functions and impl methods.
struct FrameChainVisitor<'a> {
    file_path: PathBuf,
    findings: &'a mut Vec<FrameChainFinding>,
}

impl<'a, 'ast> Visit<'ast> for FrameChainVisitor<'a> {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.check_function(&node.sig.ident.to_string(), &node.block);
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.check_function(&node.sig.ident.to_string(), &node.block);
        syn::visit::visit_impl_item_fn(self, node);
    }
}

impl<'a> FrameChainVisitor<'a> {
    /// Check a single function body for unguarded Vec locals across trampoline calls.
    fn check_function(&mut self, fn_name: &str, block: &syn::Block) {
        // Fast path: stringify the body and check for eval_trampoline_generic.
        // If the function never calls the trampoline, skip it entirely.
        let body_str = block.to_token_stream().to_string();
        if !body_str.contains("eval_trampoline_generic") {
            return;
        }

        // Walk the function body to collect Vec locals, frame guards, and trampoline calls.
        let mut walker = FnBodyWalker::default();
        walker.visit_block(block);

        // If no Vec locals with MettaValue-compatible types, nothing to check.
        if walker.vec_locals.is_empty() {
            return;
        }

        // If no trampoline calls found (shouldn't happen given the fast path, but be safe).
        if walker.trampoline_calls.is_empty() {
            return;
        }

        // For each Vec local, check if it is:
        //   (a) Declared BEFORE a trampoline call (i.e., live at the call site), AND
        //   (b) Not covered by a maybe_push_frame guard.
        //
        // Vec locals declared AFTER all trampoline calls are safe — they're not
        // live during any safepoint.
        let first_trampoline_line = walker
            .trampoline_calls
            .iter()
            .copied()
            .min()
            .expect("trampoline_calls is non-empty");

        for vec_local in &walker.vec_locals {
            // A Vec declared after the last trampoline call is safe — no
            // safepoint can fire between its creation and use.
            // More precisely: if the Vec is declared after ALL trampoline calls,
            // it's definitely safe. If it's declared before at least one
            // trampoline call, it's potentially live at that call site.
            let any_trampoline_after_vec = walker
                .trampoline_calls
                .iter()
                .any(|&tc_line| tc_line > vec_local.line);

            if !any_trampoline_after_vec {
                continue; // Vec is created after all trampoline calls — safe.
            }

            let is_covered = walker.frame_guards.iter().any(|guard| {
                // A guard covers a Vec local if it protects that variable name
                // and appears before (or at) the first trampoline call after the Vec.
                guard.protected_var == vec_local.name && guard.line <= first_trampoline_line
            });

            if !is_covered {
                // Find the first trampoline call after this Vec's declaration.
                let relevant_trampoline = walker
                    .trampoline_calls
                    .iter()
                    .copied()
                    .filter(|&tc| tc > vec_local.line)
                    .min()
                    .unwrap_or(first_trampoline_line);

                self.findings.push(FrameChainFinding {
                    file: self.file_path.clone(),
                    line: vec_local.line,
                    function_name: fn_name.to_string(),
                    vec_local: vec_local.name.clone(),
                    vec_type: vec_local.ty.clone(),
                    trampoline_call_line: relevant_trampoline,
                });
            }
        }
    }
}

/// A Vec local binding with a MettaValue-compatible element type.
#[derive(Debug)]
struct VecLocal {
    name: String,
    ty: String,
    line: usize,
}

/// A maybe_push_frame guard call and the variable it protects.
#[derive(Debug)]
struct FrameGuard {
    protected_var: String,
    line: usize,
}

/// Walks a function body collecting Vec locals, frame guards, and trampoline calls.
#[derive(Default)]
struct FnBodyWalker {
    /// Vec locals whose element type could be MettaValue.
    vec_locals: Vec<VecLocal>,
    /// maybe_push_frame guards and the variables they protect.
    frame_guards: Vec<FrameGuard>,
    /// Line numbers of eval_trampoline_generic calls.
    trampoline_calls: Vec<usize>,
}

impl<'ast> Visit<'ast> for FnBodyWalker {
    fn visit_local(&mut self, node: &'ast syn::Local) {
        // Extract the variable name and optional type annotation from the pattern.
        // syn parses `let name: Type = init;` as Pat::Type { pat: Pat::Ident, ty: Type }
        // and `let name = init;` as Pat::Ident { ident: name }.
        let (var_name, explicit_ty) = match &node.pat {
            syn::Pat::Ident(pat_ident) => {
                (Some(pat_ident.ident.to_string()), None)
            }
            syn::Pat::Type(pat_type) => {
                if let syn::Pat::Ident(inner_ident) = &*pat_type.pat {
                    let ty_str = pat_type.ty.to_token_stream().to_string();
                    (Some(inner_ident.ident.to_string()), Some(ty_str))
                } else {
                    (None, None)
                }
            }
            _ => (None, None),
        };

        if let Some(var_name) = var_name {
            // Check if the type annotation or initializer matches a MettaValue-carrying Vec.
            let is_metta_vec = if let Some(ref ty_str) = explicit_ty {
                // Explicit type annotation — check it directly.
                is_metta_value_vec_type(ty_str)
            } else if let Some(local_init) = &node.init {
                // No type annotation — check if the initializer is a known Vec-returning function.
                let init_str = local_init.expr.to_token_stream().to_string();
                is_metta_value_vec_initializer(&init_str)
            } else {
                false
            };

            if is_metta_vec {
                self.vec_locals.push(VecLocal {
                    name: var_name,
                    ty: explicit_ty.unwrap_or_else(|| node.to_token_stream().to_string()),
                    line: span_line(node.span()),
                });
            }
        }

        syn::visit::visit_local(self, node);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        let call_str = node.func.to_token_stream().to_string();

        // Detect eval_trampoline_generic calls
        if call_str.contains("eval_trampoline_generic") {
            self.trampoline_calls.push(span_line(node.span()));
        }

        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
        // maybe_push_frame is called inside unsafe { ... } blocks.
        // Walk the block looking for the call and extract the protected variable.
        let block_str = node.block.to_token_stream().to_string();
        if block_str.contains("maybe_push_frame") {
            // Extract the protected variable from the `& variable` argument.
            // The pattern is: maybe_push_frame :: < C > ( FrameLabel :: X , & variable )
            // or: maybe_push_frame::<C>(FrameLabel::X, &variable)
            if let Some(var_name) = extract_frame_guard_var(&block_str) {
                self.frame_guards.push(FrameGuard {
                    protected_var: var_name,
                    line: span_line(node.span()),
                });
            }
        }

        syn::visit::visit_expr_unsafe(self, node);
    }
}

/// Check if a type string from a `let` binding pattern matches a MettaValue-carrying Vec.
///
/// Matches patterns like:
/// - `name : Vec < C :: Value >`
/// - `name : Vec < V >`
/// - `name : Vec < MettaValue >`
/// - `name : Vec < T >` (single uppercase letter = generic param that could be MettaValue)
fn is_metta_value_vec_type(ty_str: &str) -> bool {
    // Look for Vec< followed by a MettaValue-compatible element type
    if let Some(vec_pos) = ty_str.find("Vec") {
        let after_vec = &ty_str[vec_pos + 3..].trim_start();
        if after_vec.starts_with('<') || after_vec.starts_with("< ") {
            let inner = after_vec.trim_start_matches('<').trim_start_matches(' ');
            return is_metta_value_element_type(inner);
        }
    }
    false
}

/// Check if a let-binding initializer is known to return a MettaValue-carrying Vec.
///
/// Matches:
/// - `compile_generic ( ... )` — known to return Vec<C::Value>
fn is_metta_value_vec_initializer(init_str: &str) -> bool {
    init_str.contains("compile_generic")
}

/// Check if a type element string could be MettaValue.
///
/// Matches:
/// - `C :: Value` (generic eval context value)
/// - `V` (single uppercase letter — generic param)
/// - `MettaValue`
/// - `MettaValueInner`
fn is_metta_value_element_type(element: &str) -> bool {
    let element = element.trim();

    // C :: Value or C::Value pattern
    if element.contains(":: Value") || element.contains("::Value") {
        return true;
    }

    // Concrete MettaValue
    if element.starts_with("MettaValue") {
        return true;
    }

    // Single uppercase letter (generic param that could monomorphize to MettaValue)
    // Must be at the start and followed by non-alphanumeric (or end).
    let first_char = element.chars().next();
    if let Some(c) = first_char {
        if c.is_ascii_uppercase() {
            // Check it's a single char (or followed by non-alphanumeric like '>' or ' ')
            let rest = &element[c.len_utf8()..];
            if rest.is_empty()
                || rest.starts_with('>')
                || rest.starts_with(' ')
                || rest.starts_with(',')
            {
                return true;
            }
        }
    }

    false
}

/// Extract the protected variable name from a maybe_push_frame call string.
///
/// Expected patterns (from syn token stream):
/// - `maybe_push_frame :: < C > (FrameLabel :: Include , & expressions)`
/// - `maybe_push_frame :: < C > (FrameLabel :: AssertEqual , & items)`
///
/// Returns the variable name after the `&` in the second argument.
fn extract_frame_guard_var(call_str: &str) -> Option<String> {
    // Find the last `&` before the closing `)` — this is the reference to the protected var.
    // The pattern is: ... , & varname )
    let last_ampersand = call_str.rfind('&')?;
    let after_amp = &call_str[last_ampersand + 1..];

    // Extract the identifier: alphanumeric + underscore chars
    let var: String = after_amp
        .trim_start()
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();

    if var.is_empty() {
        None
    } else {
        Some(var)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_referenced_type_names_simple() {
        let refs = extract_referenced_type_names("SpaceHandle");
        assert_eq!(refs, HashSet::from(["SpaceHandle".to_string()]));
    }

    #[test]
    fn test_extract_referenced_type_names_with_container() {
        let refs = extract_referenced_type_names("Arc<AtomSpace<MettaValue>>");
        assert!(refs.contains("AtomSpace"));
        assert!(refs.contains("MettaValue"));
        assert!(!refs.contains("Arc")); // container stripped
    }

    #[test]
    fn test_extract_referenced_type_names_dashmap() {
        let refs = extract_referenced_type_names("DashMap<String, SpaceHandle>");
        assert!(refs.contains("SpaceHandle"));
        assert!(!refs.contains("DashMap")); // container stripped
        assert!(!refs.contains("String")); // primitive stripped
    }

    #[test]
    fn test_extract_referenced_type_names_primitives_excluded() {
        let refs = extract_referenced_type_names("Vec<u64>");
        assert!(refs.is_empty());
    }

    #[test]
    fn test_extract_referenced_type_names_deeply_nested() {
        let refs = extract_referenced_type_names(
            "LazyLock<RwLock<LruCache<u64, Arc<BytecodeChunk>>>>",
        );
        assert!(refs.contains("BytecodeChunk"));
        assert!(!refs.contains("LazyLock"));
        assert!(!refs.contains("RwLock"));
        assert!(!refs.contains("LruCache"));
        assert!(!refs.contains("Arc"));
    }

    #[test]
    fn test_extract_referenced_type_names_multiple_types() {
        let refs =
            extract_referenced_type_names("DashMap<u64, Arc<ExprCompilationState>>");
        assert!(refs.contains("ExprCompilationState"));
        assert!(!refs.contains("DashMap"));
    }

    #[test]
    fn test_wrapper_fn_detection() {
        let code = r#"
            static GLOBAL_CACHE: LazyLock<Cache> = LazyLock::new(|| Cache::new());

            pub fn global_cache() -> &'static Cache {
                &GLOBAL_CACHE
            }

            static UNRELATED: AtomicBool = AtomicBool::new(false);

            pub fn get_flag() -> bool {
                UNRELATED.load(Ordering::Relaxed)
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let static_names = vec!["GLOBAL_CACHE".to_string(), "UNRELATED".to_string()];
        let mut found = HashMap::new();

        let mut visitor = WrapperFnVisitor {
            static_names: &static_names,
            found: &mut found,
        };
        visitor.visit_file(&file);

        assert_eq!(found.len(), 1, "should detect exactly one wrapper function");
        assert_eq!(
            found.get("global_cache"),
            Some(&"GLOBAL_CACHE".to_string()),
            "global_cache should map to GLOBAL_CACHE"
        );
        // get_flag doesn't use & reference pattern — shouldn't be detected
        assert!(
            !found.contains_key("get_flag"),
            "get_flag is not a wrapper function"
        );
    }

    #[test]
    fn test_data_access_detection_exposing() {
        // Type with a method that returns V (a MettaValue-carrying generic)
        let code = r#"
            struct MyCache<V> {
                items: Vec<V>,
            }

            impl<V> MyCache<V> {
                fn get(&self, idx: usize) -> Option<&V> {
                    self.items.get(idx)
                }
                fn len(&self) -> usize {
                    self.items.len()
                }
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut direct_holders = HashMap::new();
        direct_holders.insert("MyCache".to_string(), vec!["V".to_string()]);
        let mut exposing = HashSet::new();

        let mut visitor = DataAccessVisitor {
            direct_holders: &direct_holders,
            exposing: &mut exposing,
        };
        visitor.visit_file(&file);

        assert!(
            exposing.contains("MyCache"),
            "MyCache should be classified as exposing (returns Option<&V>)"
        );
    }

    #[test]
    fn test_data_access_detection_opaque() {
        // Type with NO methods that return MettaValue or V
        let code = r#"
            struct GcSnapshot {
                roots: Vec<MettaValue>,
                epoch: u64,
            }

            unsafe impl Send for GcSnapshot {}
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut direct_holders = HashMap::new();
        direct_holders.insert("GcSnapshot".to_string(), vec![]);
        let mut exposing = HashSet::new();

        let mut visitor = DataAccessVisitor {
            direct_holders: &direct_holders,
            exposing: &mut exposing,
        };
        visitor.visit_file(&file);

        assert!(
            !exposing.contains("GcSnapshot"),
            "GcSnapshot should NOT be classified as exposing (no methods return MettaValue)"
        );
    }

    #[test]
    fn test_wrapper_fn_resolves_in_root_provider() {
        let code = r#"
            static GLOBAL_FOO: LazyLock<Foo> = LazyLock::new(|| Foo::new());

            pub fn global_foo() -> &'static Foo {
                &GLOBAL_FOO
            }

            impl RootProvider for FooRoots {
                fn collect_roots(&self) -> Vec<*const u8> {
                    let foo = global_foo();
                    foo.collect()
                }
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let static_names = vec!["GLOBAL_FOO".to_string()];

        // First, build wrapper_fn_to_static
        let mut wrapper_map = HashMap::new();
        let mut wrapper_visitor = WrapperFnVisitor {
            static_names: &static_names,
            found: &mut wrapper_map,
        };
        wrapper_visitor.visit_file(&file);
        assert_eq!(wrapper_map.get("global_foo"), Some(&"GLOBAL_FOO".to_string()));

        // Then, run RootProviderImplVisitor with the wrapper map
        let mut rp_found: HashMap<String, HashSet<String>> = HashMap::new();
        let mut rp_visitor = RootProviderImplVisitor {
            static_names: &static_names,
            wrapper_fn_to_static: &wrapper_map,
            found: &mut rp_found,
        };
        rp_visitor.visit_file(&file);

        assert!(
            rp_found.get("FooRoots").map_or(false, |s| s.contains("GLOBAL_FOO")),
            "FooRoots should cover GLOBAL_FOO via wrapper function resolution"
        );
    }

    // =========================================================================
    // Frame chain safety checker tests
    // =========================================================================

    #[test]
    fn test_frame_chain_detects_unguarded_vec() {
        // Dangerous pattern: Vec<C::Value> live across eval_trampoline_generic
        // without maybe_push_frame guard.
        let code = r#"
            fn some_function(items: Vec<String>, env: Env, ctx: &Ctx) {
                let expressions: Vec<C::Value> = compile_generic(&source, factory);
                for expr in expressions.iter() {
                    let (results, new_env) = eval_trampoline_generic(expr.clone(), env, ctx);
                }
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut findings = Vec::new();
        let mut visitor = FrameChainVisitor {
            file_path: PathBuf::from("test.rs"),
            findings: &mut findings,
        };
        visitor.visit_file(&file);

        assert_eq!(findings.len(), 1, "should detect exactly one unguarded Vec");
        assert_eq!(findings[0].vec_local, "expressions");
        assert_eq!(findings[0].function_name, "some_function");
    }

    #[test]
    fn test_frame_chain_accepts_guarded_vec() {
        // Safe pattern: Vec protected by maybe_push_frame before trampoline call.
        let code = r#"
            fn safe_function(env: Env, ctx: &Ctx) {
                let expressions: Vec<C::Value> = compile_generic(&source, factory);
                let _guard = unsafe {
                    maybe_push_frame::<C>(FrameLabel::Include, &expressions)
                };
                for expr in expressions.iter() {
                    let (results, new_env) = eval_trampoline_generic(expr.clone(), env, ctx);
                }
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut findings = Vec::new();
        let mut visitor = FrameChainVisitor {
            file_path: PathBuf::from("test.rs"),
            findings: &mut findings,
        };
        visitor.visit_file(&file);

        assert!(
            findings.is_empty(),
            "should NOT flag guarded Vec — found: {:?}",
            findings
        );
    }

    #[test]
    fn test_frame_chain_ignores_no_vec() {
        // Safe pattern: trampoline call but no local Vec to protect.
        let code = r#"
            fn no_vec_function(sub_expr: Value, env: Env, ctx: &Ctx) {
                let (results, _) = eval_trampoline_generic(sub_expr.clone(), env, ctx);
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut findings = Vec::new();
        let mut visitor = FrameChainVisitor {
            file_path: PathBuf::from("test.rs"),
            findings: &mut findings,
        };
        visitor.visit_file(&file);

        assert!(
            findings.is_empty(),
            "should NOT flag function with no Vec local"
        );
    }

    #[test]
    fn test_frame_chain_detects_vec_v_generic() {
        // Vec<V> where V is a single uppercase letter (could be MettaValue).
        let code = r#"
            fn generic_function(env: Env, ctx: &Ctx) {
                let items: Vec<V> = get_items();
                let (results, _) = eval_trampoline_generic(items[0].clone(), env, ctx);
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut findings = Vec::new();
        let mut visitor = FrameChainVisitor {
            file_path: PathBuf::from("test.rs"),
            findings: &mut findings,
        };
        visitor.visit_file(&file);

        assert_eq!(findings.len(), 1, "should detect Vec<V> as potentially dangerous");
        assert_eq!(findings[0].vec_local, "items");
    }

    #[test]
    fn test_frame_chain_ignores_vec_string() {
        // Vec<String> — not a MettaValue type, should not be flagged.
        let code = r#"
            fn string_function(env: Env, ctx: &Ctx) {
                let names: Vec<String> = get_names();
                let (results, _) = eval_trampoline_generic(expr.clone(), env, ctx);
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut findings = Vec::new();
        let mut visitor = FrameChainVisitor {
            file_path: PathBuf::from("test.rs"),
            findings: &mut findings,
        };
        visitor.visit_file(&file);

        assert!(
            findings.is_empty(),
            "should NOT flag Vec<String> — not a MettaValue type"
        );
    }

    #[test]
    fn test_frame_chain_detects_compile_generic_initializer() {
        // Vec initialized from compile_generic (known to return Vec<C::Value>)
        // even without explicit type annotation.
        let code = r#"
            fn compile_function(env: Env, ctx: &Ctx) {
                let expressions = compile_generic(&source, factory);
                let (results, _) = eval_trampoline_generic(expressions[0].clone(), env, ctx);
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut findings = Vec::new();
        let mut visitor = FrameChainVisitor {
            file_path: PathBuf::from("test.rs"),
            findings: &mut findings,
        };
        visitor.visit_file(&file);

        assert_eq!(
            findings.len(),
            1,
            "should detect Vec from compile_generic as dangerous"
        );
        assert_eq!(findings[0].vec_local, "expressions");
    }

    #[test]
    fn test_extract_frame_guard_var_standard() {
        let call = "maybe_push_frame :: < C > (FrameLabel :: Include , & expressions)";
        assert_eq!(
            extract_frame_guard_var(call),
            Some("expressions".to_string())
        );
    }

    #[test]
    fn test_extract_frame_guard_var_items() {
        let call = "maybe_push_frame :: < C > (FrameLabel :: AssertEqual , & items)";
        assert_eq!(extract_frame_guard_var(call), Some("items".to_string()));
    }

    #[test]
    fn test_is_metta_value_element_type() {
        assert!(is_metta_value_element_type("C :: Value"));
        assert!(is_metta_value_element_type("V"));
        assert!(is_metta_value_element_type("MettaValue"));
        assert!(is_metta_value_element_type("T"));
        assert!(!is_metta_value_element_type("String"));
        assert!(!is_metta_value_element_type("usize"));
        assert!(!is_metta_value_element_type("u8"));
    }

    #[test]
    fn test_frame_chain_ignores_vec_after_trampoline() {
        // Vec created AFTER the trampoline call — no safepoint risk.
        // This is the pattern used in assertEqualToResult where expected_results
        // is extracted from the already-guarded `items` after the trampoline returns.
        let code = r#"
            fn assert_equal_to_result(items: Vec<C::Value>, env: Env, ctx: &Ctx) {
                let _guard = unsafe {
                    maybe_push_frame::<C>(FrameLabel::AssertEqualToResult, &items)
                };
                let (actual_results, env) = eval_trampoline_generic(items[1].clone(), env, ctx);
                let expected_results: Vec<C::Value> = match items[2].as_sexpr() {
                    Some(children) => children.to_vec(),
                    None => vec![items[2].clone()],
                };
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut findings = Vec::new();
        let mut visitor = FrameChainVisitor {
            file_path: PathBuf::from("test.rs"),
            findings: &mut findings,
        };
        visitor.visit_file(&file);

        assert!(
            findings.is_empty(),
            "should NOT flag Vec created after trampoline call — found: {:?}",
            findings
        );
    }

    #[test]
    fn test_frame_chain_impl_method() {
        // Test that impl methods are also checked.
        let code = r#"
            struct Evaluator;

            impl Evaluator {
                fn eval_with_vec(&self, env: Env, ctx: &Ctx) {
                    let items: Vec<V> = get_items();
                    let (results, _) = eval_trampoline_generic(items[0].clone(), env, ctx);
                }
            }
        "#;
        let file = syn::parse_file(code).expect("test code should parse");
        let mut findings = Vec::new();
        let mut visitor = FrameChainVisitor {
            file_path: PathBuf::from("test.rs"),
            findings: &mut findings,
        };
        visitor.visit_file(&file);

        assert_eq!(findings.len(), 1, "should detect unguarded Vec in impl method");
        assert_eq!(findings[0].function_name, "eval_with_vec");
    }

    #[test]
    fn test_raw_ptr_only_reference_const() {
        assert!(type_references_metta_value_only_via_raw_ptr(
            "RefCell < Vec < * const MettaValueInner > >"
        ));
    }

    #[test]
    fn test_raw_ptr_only_reference_mut() {
        assert!(type_references_metta_value_only_via_raw_ptr(
            "Vec < * mut MettaValueInner >"
        ));
    }

    #[test]
    fn test_not_raw_ptr_owned() {
        assert!(!type_references_metta_value_only_via_raw_ptr(
            "Vec < MettaValue >"
        ));
    }

    #[test]
    fn test_not_raw_ptr_mixed() {
        // One raw pointer + one owned — NOT raw-ptr-only
        assert!(!type_references_metta_value_only_via_raw_ptr(
            "( * const MettaValueInner , MettaValue )"
        ));
    }

    #[test]
    fn test_not_raw_ptr_no_reference() {
        assert!(!type_references_metta_value_only_via_raw_ptr("Vec < u64 >"));
    }

    #[test]
    fn test_raw_ptr_bare() {
        assert!(type_references_metta_value_only_via_raw_ptr(
            "* const MettaValueInner"
        ));
    }
}
