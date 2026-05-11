//! Pass 2: Classify MettaValue storage locations by GC safety
//!
//! ALL classification is dynamic — derived from the AST scan results.
//! No hardcoded safe lists. If a type implements RootProvider (found via AST),
//! it's registered. If not, it's flagged.
//!
//! ## Semantic Bound Propagation
//!
//! This module performs proper semantic analysis on generic type parameters:
//!
//! 1. **Seed**: For each type, identify which specific generic parameters have
//!    `MettaValueTrait` bounds. E.g., `GenericGroundedState<V: MettaValueTrait>`
//!    → param "V" carries MettaValue.
//!
//! 2. **Propagate**: If type `Foo<V>` has param "V" carrying MettaValue, and type
//!    `Bar<X, Y>` has a field `foo: Foo<X>`, then `Bar`'s param "X" also carries
//!    MettaValue (because it flows into `Foo`'s MettaValue-carrying position).
//!    This propagates transitively through the type graph to a fixed point.
//!
//! 3. **Concrete MettaValue**: Types using concrete `MettaValue` (not via generic)
//!    are classified based on usage context (persistent storage vs transient).
//!
//! The generic parameter name (`V`, `T`, `Val`, etc.) is IRRELEVANT — only the
//! semantic bounds matter.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::scanner::{extract_referenced_type_names, ScanResult, StaticKind};

/// Safety category for a MettaValue storage location
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// Type implements RootProvider — found via `impl RootProvider for X` in AST
    RegisteredRoot,
    /// The struct carries MettaValue through a semantically-bounded generic parameter
    /// (propagated transitively via MettaValueTrait bounds), OR is a borrow/reference.
    /// These are eval-context types collected during safepoints.
    GenericEvalType,
    /// Stored in static/LazyLock/OnceLock/thread_local AND not registered
    PersistentUnregistered,
    /// Transitively holds MettaValue but only through opaque types that never
    /// expose MettaValue to callers (e.g., GC snapshot data for mark-sweep pointer
    /// extraction). No use-after-free risk from GC — informational only.
    GcInfrastructure,
    /// Function-local Vec<MettaValue> is live across an eval_trampoline_generic call
    /// without maybe_push_frame protection. GC safepoint could free referenced values.
    FrameChainMissing,
    /// A MettaValue-bearing field of a RootProvider type is NOT referenced at all
    /// in collect_roots(). GC will not see these values → use-after-free.
    FieldNotCollected,
    /// A MettaValue-bearing field is accessed in collect_roots() but not all
    /// sub-fields of the iterated type are covered (e.g., only e.lhs/e.rhs
    /// but not e.rhs_type). May be intentional — needs review.
    FieldPartiallyCollected,
    /// Cannot be automatically classified — needs manual review
    Unknown,
}

/// A classified MettaValue storage location
#[derive(Debug, Clone)]
pub struct ClassifiedLocation {
    pub file: String,
    pub line: usize,
    pub context: LocationContext,
    pub category: Category,
    pub reason: String,
}

/// Context describing where a MettaValue is stored
#[derive(Debug, Clone)]
pub enum LocationContext {
    StaticVar {
        name: String,
        ty: String,
    },
    StructField {
        struct_name: String,
        field_name: String,
        ty: String,
    },
    ThreadLocal {
        name: String,
        ty: String,
    },
    TypeAlias {
        name: String,
        target: String,
    },
}

/// Classify all MettaValue storage locations found during scanning.
pub fn classify(scan: &ScanResult) -> Vec<ClassifiedLocation> {
    let mut locations = Vec::new();

    // =========================================================================
    // Phase 1: Build the root provider type set
    // =========================================================================
    let root_provider_types: &HashSet<String> = &scan.root_provider_types;

    let mut types_with_root_impl: HashSet<String> = HashSet::new();
    for ty in root_provider_types {
        types_with_root_impl.insert(ty.clone());
    }
    for imp in &scan.impl_blocks {
        if imp.has_registration {
            let base = extract_base_type(&imp.self_ty);
            types_with_root_impl.insert(base);
        }
    }

    // =========================================================================
    // Phase 2: Build covered statics set (VERIFIED registration chain)
    //
    // A data static is "covered" only if BOTH steps are verified:
    //   Step A — A collect_roots() impl method references the data static
    //   Step B — register_root_provider() is called somewhere that registers
    //            that provider type
    // =========================================================================
    let mut covered_statics: HashSet<String> = HashSet::new();

    // Chain A: Verified RootProvider → collect_roots → static
    // For each verified provider type, mark the statics it collects as covered.
    for (provider_type, referenced_statics) in &scan.statics_in_root_provider_impls {
        let base_type = extract_base_type(provider_type);
        if scan.verified_registered_providers.contains(&base_type)
            || scan.verified_registered_providers.contains(provider_type)
        {
            for static_name in referenced_statics {
                covered_statics.insert(static_name.clone());
            }
        }
    }

    // Chain B: Free function collect_*_roots (existing — unchanged)
    for name in &scan.statics_in_root_collectors {
        covered_statics.insert(name.clone());
    }

    // Chain C: Instance-level providers (impl blocks with has_registration)
    // Types where register_root_provider is called in a constructor/method of the
    // same type — these are instance-level roots (e.g., MettaState). The statics
    // they reference (if any) are covered.
    for imp in &scan.impl_blocks {
        if imp.has_registration {
            let base = extract_base_type(&imp.self_ty);
            // If this type also implements RootProvider, mark its referenced statics
            if let Some(statics) = scan.statics_in_root_provider_impls.get(&base) {
                for static_name in statics {
                    covered_statics.insert(static_name.clone());
                }
            }
        }
    }

    // Also cover statics whose type IS a RootProvider (e.g., OnceLock<Arc<dyn RootProvider>>).
    // These are the provider holder statics themselves — not data statics.
    for s in &scan.statics {
        if s.ty.contains("dyn RootProvider") || s.ty.contains("RootProvider") {
            covered_statics.insert(s.name.clone());
        }
    }

    // =========================================================================
    // Phase 3: Semantic bound propagation
    //
    // For each type, determine WHICH of its generic parameters carry MettaValue.
    // Start with explicit MettaValueTrait bounds, then propagate through fields.
    // =========================================================================

    // Step 3a: Seed — types with explicit MettaValueTrait bounds on specific params.
    // type_metta_params: type_name → set of param names that carry MettaValue
    let mut type_metta_params: HashMap<String, HashSet<String>> = HashMap::new();
    for (type_name, bounds) in &scan.generic_bounds {
        for bound in bounds {
            if bound.bound.contains("MettaValueTrait") {
                type_metta_params
                    .entry(type_name.clone())
                    .or_default()
                    .insert(bound.param.clone());
            }
        }
    }

    // Merge function-scope bounds (from free functions and impl blocks).
    // These capture MettaValueTrait bounds that exist at function/impl scope
    // rather than on the type definition itself.
    for (type_name, bounds) in &scan.function_scope_bounds {
        for bound in bounds {
            if bound.bound.contains("MettaValueTrait") {
                type_metta_params
                    .entry(type_name.clone())
                    .or_default()
                    .insert(bound.param.clone());
            }
        }
    }

    // Build lookup: type_name → ordered list of type params
    let type_params_ordered: HashMap<&str, &[String]> = scan
        .type_defs
        .iter()
        .map(|td| (td.name.as_str(), td.type_params.as_slice()))
        .collect();

    // Step 3b: Bidirectional propagation through field type references.
    //
    // Two directions of propagation, iterated to a fixed point:
    //
    // **Downward (field → parent)**: If type T has field `f: C<arg>` where C is a
    //   known carrier with carrier param at position i, and `arg` matches T's param
    //   Pj, then T's Pj becomes a carrier. (Parent inherits carrier status from field.)
    //
    // **Upward (parent → field)**: If type T is a known carrier with carrier param P,
    //   and T has field `f: U<P>` where U has param Q at the position P occupies in
    //   the type arg list, then U's Q also becomes a carrier. (Field type inherits
    //   carrier status from parent passing its carrier param.)
    //
    // Both directions are needed because:
    // - Downward handles `GenericContinuation<V>` inheriting from `GenericGroundedState<V>` field
    // - Upward handles `GenericTokenizer<V>` (no bound) inheriting from
    //   `GenericEnvironmentShared<V>` (carrier) having field `tokenizer: ...GenericTokenizer<V>...`
    let mut changed = true;
    while changed {
        changed = false;
        let snapshot = type_metta_params.clone();
        for type_def in &scan.type_defs {
            let type_name = &type_def.name;
            let my_params = &type_def.type_params;

            for field in &type_def.fields {
                // === Downward propagation ===
                // Check if field references an already-known carrier type
                for (carrier_name, carrier_metta_params) in &snapshot {
                    if !field.ty.contains(carrier_name.as_str()) {
                        continue;
                    }

                    if let Some(args) = extract_type_args(&field.ty, carrier_name) {
                        if let Some(carrier_params) = type_params_ordered.get(carrier_name.as_str())
                        {
                            for carrier_param in carrier_metta_params {
                                if let Some(pos) =
                                    carrier_params.iter().position(|p| p == carrier_param)
                                {
                                    if let Some(arg) = args.get(pos) {
                                        let arg_trimmed = arg.trim();
                                        if my_params.contains(&arg_trimmed.to_string()) {
                                            let entry = type_metta_params
                                                .entry(type_name.clone())
                                                .or_default();
                                            if entry.insert(arg_trimmed.to_string()) {
                                                changed = true;
                                            }
                                        }
                                        if arg_trimmed == "MettaValue" {
                                            let entry = type_metta_params
                                                .entry(type_name.clone())
                                                .or_default();
                                            if entry.insert("__concrete__".to_string()) {
                                                changed = true;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // === Upward propagation ===
                // If THIS type is a known carrier, propagate carrier status to types
                // referenced in its fields by passing carrier params as type arguments.
                if let Some(my_carrier_params) = snapshot.get(type_name.as_str()) {
                    // Find all type_defs types referenced in this field
                    for target_td in &scan.type_defs {
                        let target_name = &target_td.name;
                        if target_name == type_name {
                            continue; // skip self-references
                        }
                        if !field.ty.contains(target_name.as_str()) {
                            continue;
                        }
                        if let Some(args) = extract_type_args(&field.ty, target_name) {
                            let target_params = &target_td.type_params;
                            // For each arg position, if the arg is one of our carrier
                            // params, then the target's param at that position is a carrier
                            for (i, arg) in args.iter().enumerate() {
                                let arg_trimmed = arg.trim();
                                if my_carrier_params.contains(arg_trimmed) {
                                    if let Some(target_param) = target_params.get(i) {
                                        let entry = type_metta_params
                                            .entry(target_name.clone())
                                            .or_default();
                                        if entry.insert(target_param.clone()) {
                                            changed = true;
                                        }
                                    }
                                }
                                // Also handle concrete MettaValue being passed
                                if arg_trimmed == "MettaValue" {
                                    if let Some(target_param) = target_params.get(i) {
                                        let entry = type_metta_params
                                            .entry(target_name.clone())
                                            .or_default();
                                        if entry.insert(target_param.clone()) {
                                            changed = true;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Build the set of types that are semantically-proven MettaValue carriers
    let carrier_types: HashSet<&str> = type_metta_params.keys().map(|k| k.as_str()).collect();

    // =========================================================================
    // Phase 3b: Transitive type reachability
    //
    // Some statics hold MettaValue *transitively* through concrete types whose
    // names don't mention "MettaValue". E.g.:
    //
    //   GLOBAL_SPACE_REGISTRY: LazyLock<SpaceRegistry>
    //     → SpaceRegistry.spaces: DashMap<String, SpaceHandle>
    //       → SpaceHandle.backing: SpaceBacking
    //         → SpaceBacking::Owned { space: Arc<AtomSpace<MettaValue>> }
    //
    // We build a reachability set of type names that transitively contain
    // MettaValue through field type references.
    //
    // Seed: types that directly reference MettaValue/MettaValueInner in a field
    // Propagate: if type A has a field referencing type B, and B is reachable,
    //            then A is reachable. Fixed-point iteration.
    // =========================================================================
    let mut types_reaching_metta_value: HashSet<String> = HashSet::new();

    // Seed with types that directly reference MettaValue in their fields
    // (from the type_defs that the scanner collected — these all have at least
    // one field containing MettaValue)
    for type_def in &scan.type_defs {
        for field in &type_def.fields {
            if field.contains_metta_value {
                types_reaching_metta_value.insert(type_def.name.clone());
                break;
            }
        }
    }

    // Also seed from type_field_references: any type that directly references
    // "MettaValue" or "MettaValueInner" in its field types
    for (type_name, referenced_types) in &scan.type_field_references {
        for ref_ty in referenced_types {
            if ref_ty == "MettaValue" || ref_ty == "MettaValueInner" || ref_ty == "MettaValueTrait"
            {
                types_reaching_metta_value.insert(type_name.clone());
                break;
            }
        }
    }

    // Also seed from type aliases: if a type alias target references MettaValue,
    // the alias name is a MettaValue carrier. E.g., `type BytecodeChunk = GenericBytecodeChunk<MettaValue>`
    for alias in &scan.type_aliases {
        types_reaching_metta_value.insert(alias.name.clone());
    }

    // Fixed-point propagation through type_field_references graph
    let mut changed = true;
    while changed {
        changed = false;
        for (type_name, referenced_types) in &scan.type_field_references {
            if types_reaching_metta_value.contains(type_name) {
                continue;
            }
            for ref_ty in referenced_types {
                if types_reaching_metta_value.contains(ref_ty) {
                    types_reaching_metta_value.insert(type_name.clone());
                    changed = true;
                    break;
                }
            }
        }
    }

    // =========================================================================
    // Phase 3c: Data-access reachability
    //
    // Not all types that transitively hold MettaValue actually expose it to
    // callers. Types like GcSnapshot hold MettaValue solely for GC pointer
    // extraction (`.inner_ptr()`) — external code never dereferences the data.
    //
    // We build a second reachability set limited to types that expose MettaValue
    // through method return types ("data access"). This distinguishes:
    //   - Data types (statics that need GC root registration)
    //   - GC infrastructure (statics that hold MettaValue opaquely)
    //
    // Seed: direct holder types in `scan.types_exposing_metta_value` +
    //       type aliases (the alias name + the generic type they monomorphize)
    //
    // IMPORTANT: We do NOT seed bare "MettaValue"/"MettaValueInner" into the
    // data set. These names appear in type_field_references for EVERY holder
    // type, which would defeat the purpose of distinguishing data vs opaque.
    // Instead, we propagate through named user types only.
    // =========================================================================
    let mut types_reaching_metta_value_as_data: HashSet<String> = HashSet::new();

    // Seed from types_exposing_metta_value (direct holders with data-returning methods)
    for type_name in &scan.types_exposing_metta_value {
        types_reaching_metta_value_as_data.insert(type_name.clone());
    }

    // Also seed from type aliases: the alias name AND the generic type it instantiates.
    // E.g., `type BytecodeChunk = GenericBytecodeChunk<MettaValue>` seeds both
    // "BytecodeChunk" and "GenericBytecodeChunk". But NOT bare "MettaValue".
    for alias in &scan.type_aliases {
        types_reaching_metta_value_as_data.insert(alias.name.clone());
        // Extract the generic type name from the alias target
        // (the first PascalCase identifier that isn't MettaValue/MettaValueInner)
        for ref_name in extract_referenced_type_names(&alias.target) {
            if ref_name != "MettaValue"
                && ref_name != "MettaValueInner"
                && ref_name != "MettaValueTrait"
            {
                types_reaching_metta_value_as_data.insert(ref_name);
            }
        }
    }

    // Also seed types that directly reference MettaValue/MettaValueInner in fields
    // AND are in the exposing set (these are the data-carrying roots)
    for type_def in &scan.type_defs {
        if scan.types_exposing_metta_value.contains(&type_def.name) {
            types_reaching_metta_value_as_data.insert(type_def.name.clone());
        }
    }

    // Fixed-point propagation through type_field_references graph.
    // A type reaches MettaValue as data if it references a type in the data set.
    // We exclude bare "MettaValue"/"MettaValueInner" from triggering propagation
    // because they appear in type_field_references for ALL holder types.
    let mut changed = true;
    while changed {
        changed = false;
        for (type_name, referenced_types) in &scan.type_field_references {
            if types_reaching_metta_value_as_data.contains(type_name) {
                continue;
            }
            // Only propagate if the type also reaches MettaValue at all
            if !types_reaching_metta_value.contains(type_name) {
                continue;
            }
            for ref_ty in referenced_types {
                // Skip bare MettaValue/MettaValueInner — they propagate to everything
                if ref_ty == "MettaValue"
                    || ref_ty == "MettaValueInner"
                    || ref_ty == "MettaValueTrait"
                {
                    continue;
                }
                if types_reaching_metta_value_as_data.contains(ref_ty) {
                    types_reaching_metta_value_as_data.insert(type_name.clone());
                    changed = true;
                    break;
                }
            }
        }
    }

    // =========================================================================
    // Phase 4: Classify statics
    //
    // A static needs registration only if it transitively holds MettaValue
    // through types that EXPOSE MettaValue to callers (data access).
    // Statics that reach MettaValue only through opaque types (e.g., GcSnapshot
    // for mark-sweep) don't need registration — no use-after-free risk.
    // =========================================================================
    for static_decl in &scan.statics {
        // Check direct MettaValue reference OR transitive reachability.
        // We check ALL type names referenced in the static's type string,
        // not just the outermost base type, to handle deeply nested wrappers like
        // LazyLock<RwLock<LruCache<u64, Arc<BytecodeChunk>>>>
        let referenced_names = extract_referenced_type_names(&static_decl.ty);

        let transitively_contains_data = referenced_names
            .iter()
            .any(|name| types_reaching_metta_value_as_data.contains(name));

        let transitively_contains_any = referenced_names
            .iter()
            .any(|name| types_reaching_metta_value.contains(name));

        // Opaque-only: reaches MettaValue but only through non-data-exposing types
        let transitively_contains_opaque_only = !static_decl.contains_metta_value
            && !transitively_contains_data
            && transitively_contains_any;

        if !static_decl.contains_metta_value && !transitively_contains_data {
            if transitively_contains_opaque_only {
                // Report as GcInfrastructure — low-priority warning
                let opaque_via = referenced_names
                    .iter()
                    .find(|name| types_reaching_metta_value.contains(*name))
                    .cloned()
                    .unwrap_or_default();

                locations.push(ClassifiedLocation {
                    file: static_decl.file.display().to_string(),
                    line: static_decl.line,
                    context: match static_decl.kind {
                        StaticKind::ThreadLocal => LocationContext::ThreadLocal {
                            name: static_decl.name.clone(),
                            ty: static_decl.ty.clone(),
                        },
                        _ => LocationContext::StaticVar {
                            name: static_decl.name.clone(),
                            ty: static_decl.ty.clone(),
                        },
                    },
                    category: Category::GcInfrastructure,
                    reason: format!(
                        "{:?} {} of type '{}' transitively holds MettaValue (via {}) but only through opaque types — no data exposure",
                        static_decl.kind, static_decl.name, static_decl.ty, opaque_via
                    ),
                });
            }
            continue;
        }

        // Raw-pointer-only: type string mentions MettaValue but only through *const/*mut.
        // Raw pointers don't hold ownership — not GC roots.
        if static_decl.only_raw_ptr_reference {
            locations.push(ClassifiedLocation {
                file: static_decl.file.display().to_string(),
                line: static_decl.line,
                context: match static_decl.kind {
                    StaticKind::ThreadLocal => LocationContext::ThreadLocal {
                        name: static_decl.name.clone(),
                        ty: static_decl.ty.clone(),
                    },
                    _ => LocationContext::StaticVar {
                        name: static_decl.name.clone(),
                        ty: static_decl.ty.clone(),
                    },
                },
                category: Category::GcInfrastructure,
                reason: format!(
                    "{:?} {} references MettaValue only via raw pointers (*const/*mut) — \
                     no ownership, not a GC root",
                    static_decl.kind, static_decl.name
                ),
            });
            continue;
        }

        // For the reason string, identify which type triggered transitive reachability
        let transitive_via = if transitively_contains_data && !static_decl.contains_metta_value {
            referenced_names
                .iter()
                .find(|name| types_reaching_metta_value_as_data.contains(*name))
                .cloned()
                .unwrap_or_default()
        } else {
            String::new()
        };

        let category = if covered_statics.contains(&static_decl.name) {
            Category::RegisteredRoot
        } else {
            Category::PersistentUnregistered
        };

        let reason = match &category {
            Category::RegisteredRoot => {
                "Covered by RootProvider registration (companion or type impl)".to_string()
            }
            Category::PersistentUnregistered => {
                if transitively_contains_data && !static_decl.contains_metta_value {
                    format!(
                        "{:?} {} of type '{}' transitively stores MettaValue (via {}) but no RootProvider found",
                        static_decl.kind, static_decl.name, static_decl.ty, transitive_via
                    )
                } else {
                    format!(
                        "{:?} {} of type '{}' stores MettaValue but no RootProvider found",
                        static_decl.kind, static_decl.name, static_decl.ty
                    )
                }
            }
            _ => String::new(),
        };

        locations.push(ClassifiedLocation {
            file: static_decl.file.display().to_string(),
            line: static_decl.line,
            context: match static_decl.kind {
                StaticKind::ThreadLocal => LocationContext::ThreadLocal {
                    name: static_decl.name.clone(),
                    ty: static_decl.ty.clone(),
                },
                _ => LocationContext::StaticVar {
                    name: static_decl.name.clone(),
                    ty: static_decl.ty.clone(),
                },
            },
            category,
            reason,
        });
    }

    // =========================================================================
    // Phase 5: Classify struct/enum fields
    //
    // Classification hierarchy (most specific → least):
    //   1. RootProvider — type implements RootProvider trait
    //   2. Borrow — field is a reference, scope-limited by borrow checker
    //   3. Carrier type — type has a param proven to carry MettaValue
    //   4. Field references carrier — field's type mentions a carrier type
    //   5. Field uses carrier param directly — field type IS a carrier param
    //      or wraps it in a stdlib container (Vec<V>, Option<V>, etc.)
    //   6. Concrete MettaValue — field uses MettaValue directly (not via generic)
    //      These are transient work items (statics handled separately in Phase 4)
    //   7. Unknown — none of the above matched
    // =========================================================================
    for type_def in &scan.type_defs {
        let type_name = &type_def.name;

        let is_root_provider = types_with_root_impl.contains(type_name);
        let is_carrier = carrier_types.contains(type_name.as_str());

        // Get this type's carrier params (if any)
        let my_carrier_params: Option<&HashSet<String>> = type_metta_params.get(type_name.as_str());

        for field in &type_def.fields {
            if !field.contains_metta_value {
                continue;
            }

            let is_borrow = field.ty.starts_with("& '")
                || field.ty.starts_with("&'")
                || field.ty.starts_with("& MettaValue")
                || field.ty.starts_with("&MettaValue");

            // Check if the field references any carrier type by name
            let field_has_carrier = carrier_types.iter().any(|c| field.ty.contains(c));

            // Check if the field's type directly uses one of this type's carrier
            // params, either bare (e.g., `field: V`) or wrapped in stdlib containers
            // (e.g., `Vec<V>`, `Option<V>`, `SmallVec<[V; 8]>`, `Arc<Vec<V>>`).
            // This handles the case where V doesn't appear as a type_defs carrier
            // name but IS a known carrier param of the enclosing type.
            let field_uses_carrier_param = my_carrier_params
                .map(|params| {
                    params.iter().any(|p| {
                        if p == "__concrete__" {
                            return false;
                        }
                        field_type_uses_param(&field.ty, p)
                    })
                })
                .unwrap_or(false);

            // Check if the field directly uses concrete "MettaValue" (not via generic).
            // Non-static structs holding concrete MettaValue are transient work items.
            let uses_concrete_metta_value = type_references_concrete_metta_value(&field.ty);

            // PhantomData is zero-sized (compile-time only) — no GC concern.
            let is_phantom = field.ty.contains("PhantomData");

            // Type is never instantiated with a MettaValue carrier in the codebase.
            let never_instantiated_with_metta = scan
                .types_never_instantiated_with_metta
                .contains(type_name.as_str());

            let category = if is_root_provider {
                Category::RegisteredRoot
            } else if is_borrow {
                Category::GenericEvalType
            } else if is_carrier {
                Category::GenericEvalType
            } else if field_has_carrier {
                Category::GenericEvalType
            } else if field_uses_carrier_param {
                Category::GenericEvalType
            } else if uses_concrete_metta_value {
                // Concrete MettaValue in a non-static type — transient work item.
                // Persistent statics are classified in Phase 4.
                Category::GenericEvalType
            } else if is_phantom {
                // PhantomData<V> is zero-sized — compile-time marker only.
                Category::GenericEvalType
            } else if never_instantiated_with_metta {
                // Generic type never concretely instantiated with MettaValue.
                Category::GenericEvalType
            } else {
                Category::Unknown
            };

            let reason = match &category {
                Category::RegisteredRoot => {
                    format!("{} implements RootProvider (found in AST)", type_name)
                }
                Category::GenericEvalType => {
                    if is_borrow {
                        format!(
                            "{}.{}: {} — borrow (scope-limited, safe by Rust borrow checker)",
                            type_name, field.name, field.ty
                        )
                    } else if is_carrier {
                        let params = type_metta_params
                            .get(type_name.as_str())
                            .map(|p| {
                                p.iter()
                                    .filter(|s| *s != "__concrete__")
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default();
                        if params.is_empty() {
                            format!(
                                "{} is a semantically-proven MettaValue carrier (concrete MettaValue in carrier-typed field)",
                                type_name
                            )
                        } else {
                            format!(
                                "{} is a semantically-proven MettaValue carrier (params [{}] bounded by MettaValueTrait, propagated transitively)",
                                type_name, params
                            )
                        }
                    } else if field_has_carrier {
                        format!(
                            "{}.{} references carrier type — eval context (collected during safepoints)",
                            type_name, field.name
                        )
                    } else if field_uses_carrier_param {
                        format!(
                            "{}.{}: {} — uses carrier param of enclosing type (transitive MettaValueTrait bound)",
                            type_name, field.name, field.ty
                        )
                    } else if is_phantom {
                        format!(
                            "{}.{}: {} — PhantomData is zero-sized (compile-time only, no GC concern)",
                            type_name, field.name, field.ty
                        )
                    } else if never_instantiated_with_metta {
                        format!(
                            "{}.{}: {} — generic type never instantiated with MettaValue carrier in codebase",
                            type_name, field.name, field.ty
                        )
                    } else {
                        format!(
                            "{}.{}: {} — concrete MettaValue in transient work item (not persistent storage)",
                            type_name, field.name, field.ty
                        )
                    }
                }
                Category::Unknown => {
                    format!(
                        "{}.{}: {} — no RootProvider impl found, needs review",
                        type_name, field.name, field.ty
                    )
                }
                _ => String::new(),
            };

            locations.push(ClassifiedLocation {
                file: type_def.file.display().to_string(),
                line: field.line,
                context: LocationContext::StructField {
                    struct_name: type_name.clone(),
                    field_name: field.name.clone(),
                    ty: field.ty.clone(),
                },
                category,
                reason,
            });
        }
    }

    // =========================================================================
    // Phase 6: Classify type aliases (informational only)
    // =========================================================================
    for alias in &scan.type_aliases {
        locations.push(ClassifiedLocation {
            file: alias.file.display().to_string(),
            line: alias.line,
            context: LocationContext::TypeAlias {
                name: alias.name.clone(),
                target: alias.target.clone(),
            },
            category: Category::RegisteredRoot,
            reason: "Type alias — not a storage location".to_string(),
        });
    }

    // =========================================================================
    // Phase 7: Frame chain safety — Vec locals live across trampoline calls
    //
    // Any function-local Vec<MettaValue> (or Vec<C::Value>, Vec<V>, etc.)
    // that is live across an eval_trampoline_generic call MUST be protected
    // by a maybe_push_frame guard. Without it, the GC safepoint inside the
    // trampoline cannot see the Vec's contents and may free referenced values.
    // =========================================================================
    for finding in &scan.frame_chain_findings {
        locations.push(ClassifiedLocation {
            file: finding.file.display().to_string(),
            line: finding.line,
            context: LocationContext::StaticVar {
                name: format!("{}::{}", finding.function_name, finding.vec_local),
                ty: finding.vec_type.clone(),
            },
            category: Category::FrameChainMissing,
            reason: format!(
                "Vec `{}` in function `{}` is live across eval_trampoline_generic call (line {}) \
                 without maybe_push_frame guard — GC safepoint could free referenced values",
                finding.vec_local, finding.function_name, finding.trampoline_call_line
            ),
        });
    }

    // =========================================================================
    // Phase 8: collect_roots() field coverage verification
    //
    // For each RegisteredRoot type that has a collect_roots_analysis, cross-
    // reference its MettaValue-bearing fields against the fields accessed
    // in collect_roots(). Flag:
    //   - FieldNotCollected: MettaValue-bearing field not referenced at all
    //   - FieldPartiallyCollected: field accessed but sub-fields of iterated
    //     type are incomplete (e.g., e.lhs, e.rhs but not e.rhs_type)
    //
    // This is the exact class of bug that caused the PLN regression:
    //   - inferred_fn_types: FieldNotCollected (not referenced in collect_roots)
    //   - rule_index → RuleEntry.rhs_type: FieldPartiallyCollected (only lhs, rhs)
    // =========================================================================
    for type_def in &scan.type_defs {
        let type_name = &type_def.name;

        // Only check types that implement RootProvider
        if !types_with_root_impl.contains(type_name) {
            continue;
        }

        // Get the collect_roots analysis for this type (or its shared inner type).
        // RootProvider is often implemented for `GenericEnvironmentShared<V>` while
        // the type_def is `GenericEnvironmentShared` — try both.
        let analysis = scan.collect_roots_analyses.get(type_name);
        let analysis = match analysis {
            Some(a) => a,
            None => continue, // No analysis available (collect_roots not found or parse error)
        };

        // Check each MettaValue-bearing field
        for field in &type_def.fields {
            if !field.contains_metta_value {
                continue;
            }

            let field_name = &field.name;

            // Check if this field is covered in collect_roots
            match analysis.covered_fields.get(field_name.as_str()) {
                None => {
                    // Field not referenced at all in collect_roots → FieldNotCollected
                    locations.push(ClassifiedLocation {
                        file: type_def.file.display().to_string(),
                        line: field.line,
                        context: LocationContext::StructField {
                            struct_name: type_name.clone(),
                            field_name: field_name.clone(),
                            ty: field.ty.clone(),
                        },
                        category: Category::FieldNotCollected,
                        reason: format!(
                            "{}.{}: {} — MettaValue-bearing field not referenced in collect_roots(). \
                             GC cannot see these values → potential use-after-free",
                            type_name, field_name, field.ty
                        ),
                    });
                }
                Some(crate::scanner::FieldCoverage::Partial(sub_fields)) => {
                    // Field accessed with specific sub-fields. Check if the field's type
                    // has additional MettaValue-bearing fields not covered.
                    // For now, report which sub-fields are covered as a warning.
                    locations.push(ClassifiedLocation {
                        file: type_def.file.display().to_string(),
                        line: field.line,
                        context: LocationContext::StructField {
                            struct_name: type_name.clone(),
                            field_name: field_name.clone(),
                            ty: field.ty.clone(),
                        },
                        category: Category::FieldPartiallyCollected,
                        reason: format!(
                            "{}.{}: {} — collect_roots() only accesses sub-fields [{}]. \
                             Other MettaValue-bearing sub-fields may be missed",
                            type_name,
                            field_name,
                            field.ty,
                            sub_fields.join(", ")
                        ),
                    });
                }
                Some(crate::scanner::FieldCoverage::Delegated)
                | Some(crate::scanner::FieldCoverage::Full) => {
                    // Fully covered — no issue
                }
            }
        }
    }

    locations
}

/// Count locations by category
pub fn count_by_category(locations: &[ClassifiedLocation]) -> BTreeMap<Category, usize> {
    let mut counts = BTreeMap::new();
    for loc in locations {
        *counts.entry(loc.category.clone()).or_insert(0) += 1;
    }
    counts
}

/// Check whether a field type string uses a generic param (with word-boundary matching).
///
/// This handles bare params (`V`), stdlib containers wrapping them (`Vec<V>`,
/// `Option<V>`, `SmallVec<[V; 8]>`, `Arc<Vec<V>>`, `HashMap<usize, Vec<V>>`),
/// tuples (`(V, GenericBindings<V>)`), and function pointer types containing them.
///
/// Uses word-boundary matching to avoid false positives like "V" inside "Vec".
fn field_type_uses_param(field_ty: &str, param: &str) -> bool {
    if param.len() == 1 {
        let param_char = param.chars().next().expect("param should be non-empty");
        let chars: Vec<char> = field_ty.chars().collect();
        for (i, &ch) in chars.iter().enumerate() {
            if ch == param_char {
                let before_ok = i == 0 || !chars[i - 1].is_alphanumeric();
                let after_ok = i + 1 >= chars.len() || !chars[i + 1].is_alphanumeric();
                if before_ok && after_ok {
                    return true;
                }
            }
        }
        false
    } else {
        field_ty.contains(param)
    }
}

/// Check whether a field type string references concrete `MettaValue` (the specific type,
/// not a generic param that might be MettaValue).
///
/// Returns true for types like `MettaValue`, `Vec<MettaValue>`, `Option<MettaValue>`,
/// `Arc<Vec<MettaValue>>`, `(String, MettaValue)`, `MettaValueInner`, etc.
fn type_references_concrete_metta_value(field_ty: &str) -> bool {
    field_ty.contains("MettaValue") || field_ty.contains("MettaValueInner")
}

/// Extract type arguments from a type string for a given type constructor.
///
/// E.g., `extract_type_args("GenericGroundedState < V >", "GenericGroundedState")`
/// → `Some(vec!["V"])`
///
/// `extract_type_args("HashMap < usize , Vec < V > >", "HashMap")`
/// → `Some(vec!["usize", "Vec < V >"])`
///
/// Handles nested angle brackets by tracking bracket depth.
fn extract_type_args(type_str: &str, constructor: &str) -> Option<Vec<String>> {
    // Find the constructor name in the type string
    let start_idx = type_str.find(constructor)?;
    let after_name = &type_str[start_idx + constructor.len()..];

    // Find the opening '<' (may have spaces)
    let trimmed = after_name.trim_start();
    if !trimmed.starts_with('<') {
        return None;
    }

    let args_start = after_name.len() - trimmed.len() + 1; // skip '<'
    let rest = &after_name[args_start..];

    // Parse arguments, tracking bracket depth (angle brackets AND parentheses)
    let mut args = Vec::new();
    let mut current_arg = String::new();
    let mut angle_depth = 1;
    let mut paren_depth = 0;
    let mut bracket_depth = 0;

    for ch in rest.chars() {
        match ch {
            '<' => {
                angle_depth += 1;
                current_arg.push(ch);
            }
            '>' => {
                angle_depth -= 1;
                if angle_depth == 0 {
                    let arg = current_arg.trim().to_string();
                    if !arg.is_empty() {
                        args.push(arg);
                    }
                    break;
                }
                current_arg.push(ch);
            }
            '(' => {
                paren_depth += 1;
                current_arg.push(ch);
            }
            ')' => {
                paren_depth -= 1;
                current_arg.push(ch);
            }
            '[' => {
                bracket_depth += 1;
                current_arg.push(ch);
            }
            ']' => {
                bracket_depth -= 1;
                current_arg.push(ch);
            }
            ',' if angle_depth == 1 && paren_depth == 0 && bracket_depth == 0 => {
                let arg = current_arg.trim().to_string();
                if !arg.is_empty() {
                    args.push(arg);
                }
                current_arg.clear();
            }
            _ => {
                current_arg.push(ch);
            }
        }
    }

    if args.is_empty() {
        None
    } else {
        Some(args)
    }
}

/// Extract the base type name from a complex type string
fn extract_base_type(ty: &str) -> String {
    let mut t = ty.to_string();
    for prefix in &[
        "Arc < ",
        "Arc<",
        "Mutex < ",
        "Mutex<",
        "RwLock < ",
        "RwLock<",
        "OnceLock < ",
        "OnceLock<",
        "LazyLock < ",
        "LazyLock<",
        "Vec < ",
        "Vec<",
        "Option < ",
        "Option<",
        "Box < ",
        "Box<",
    ] {
        if t.starts_with(prefix) {
            t = t[prefix.len()..].to_string();
            if t.ends_with('>') {
                t.pop();
            }
        }
    }
    t.split('<')
        .next()
        .unwrap_or(&t)
        .trim()
        .split("::")
        .last()
        .unwrap_or(&t)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_type_args_simple() {
        let args = extract_type_args("GenericGroundedState < V >", "GenericGroundedState");
        assert_eq!(args, Some(vec!["V".to_string()]));
    }

    #[test]
    fn test_extract_type_args_multiple() {
        let args = extract_type_args("HashMap < String , V >", "HashMap");
        assert_eq!(args, Some(vec!["String".to_string(), "V".to_string()]));
    }

    #[test]
    fn test_extract_type_args_nested() {
        let args = extract_type_args("Vec < GenericBindings < V > >", "Vec");
        assert_eq!(args, Some(vec!["GenericBindings < V >".to_string()]));
    }

    #[test]
    fn test_extract_type_args_no_params() {
        let args = extract_type_args("String", "String");
        assert_eq!(args, None);
    }

    #[test]
    fn test_extract_type_args_deeply_nested() {
        let args = extract_type_args("VecDeque < (V , GenericBindings < V >) >", "VecDeque");
        assert_eq!(args, Some(vec!["(V , GenericBindings < V >)".to_string()]));
    }
}
