# MeTTa-on-MORK Implementation Guide
**Condensed Reference: Pattern Matching, Spaces, Evaluation & Roadmap**

**Version**: 1.0
**Last Updated**: 2025-11-13

## Quick Navigation

1. [Pattern Matching Implementation](#pattern-matching-implementation)
2. [Space Operations](#space-operations)
3. [Evaluation Engine](#evaluation-engine)
4. [Implementation Roadmap](#implementation-roadmap)
5. [Common Challenges & Solutions](#common-challenges--solutions)

---

## Pattern Matching Implementation

### Core Challenge: Named Variables ↔ De Bruijn

**MeTTa**: `$x`, `$y` (named)
**MORK**: Level 0, Level 1 (positional)

**Solution - Hybrid Representation**:
```rust
pub struct PatternContext {
    // Track variable names → De Bruijn levels
    name_to_level: HashMap<String, u8>,
    level_to_name: Vec<String>,
    next_level: u8,
}

impl PatternContext {
    pub fn introduce_var(&mut self, name: &str) -> u8 {
        if let Some(&level) = self.name_to_level.get(name) {
            return level;  // Already bound
        }

        let level = self.next_level;
        self.name_to_level.insert(name.to_string(), level);
        self.level_to_name.push(name.to_string());
        self.next_level += 1;
        level
    }

    pub fn lookup_var(&self, name: &str) -> Option<u8> {
        self.name_to_level.get(name).copied()
    }

    pub fn get_name(&self, level: u8) -> Option<&str> {
        self.level_to_name.get(level as usize).map(|s| s.as_str())
    }
}
```

### Pattern → MORK Source Conversion

**Algorithm**:
```rust
pub fn pattern_to_source(pattern: &Atom) -> Result<BTMSource, ConversionError> {
    let mut ctx = PatternContext::new();
    let encoded = encode_pattern(pattern, &mut ctx)?;

    // Create BTMSource with encoded pattern
    BTMSource::new(encoded)
}

fn encode_pattern(atom: &Atom, ctx: &mut PatternContext) -> Result<Vec<u8>, EncodeError> {
    match atom {
        Atom::Symbol(s) => encode_symbol(s),

        Atom::Variable(v) => {
            // Introduce or reference variable
            let level = ctx.introduce_var(v.name());
            if is_first_occurrence(v, ctx) {
                Ok(vec![Tag::NewVar as u8])
            } else {
                Ok(encode_var_ref(level))
            }
        }

        Atom::Expression(e) => {
            let mut result = vec![e.len() as u8];
            for child in e.children() {
                result.extend(encode_pattern(child, ctx)?);
            }
            Ok(result)
        }

        Atom::Grounded(_) => {
            // Grounded atoms in patterns: exact match required
            encode_grounded(atom)
        }
    }
}
```

### MORK Results → MeTTa Bindings

**Conversion Flow**:
```
MORK PathMap Query Result (byte paths)
    ↓
Extract variable values from paths
    ↓
Map De Bruijn levels → Variable names
    ↓
Build MeTTa Bindings structure
```

**Implementation**:
```rust
pub fn mork_results_to_bindings(
    paths: impl Iterator<Item = &[u8]>,
    pattern_ctx: &PatternContext,
) -> BindingsSet {
    let mut binding_set = BindingsSet::empty();

    for path in paths {
        let mut bindings = Bindings::new();
        let mut offset = 0;

        // Parse path and extract variable bindings
        while offset < path.len() {
            let (atom, consumed) = decode_atom(&path[offset..])?;
            offset += consumed;

            // If this position corresponds to a variable...
            if let Some(var_name) = get_variable_at_position(offset, pattern_ctx) {
                let var = VariableAtom::new(var_name);
                bindings.add_var_binding(var, atom);
            }
        }

        binding_set.push(bindings);
    }

    binding_set
}
```

### Multi-Constraint Queries (Product Zippers)

**Example**: Pattern with equality constraint
```metta
; Match: (parent $x $y) where $x == "Alice"
```

**MORK Implementation**:
```rust
pub fn match_with_constraint(
    space: &PathMap,
    pattern: &Atom,
    constraints: Vec<Constraint>,
) -> BindingsSet {
    // Create sources
    let pattern_source = BTMSource::new(encode_pattern(pattern)?);

    let mut sources: Vec<Box<dyn Source>> = vec![Box::new(pattern_source)];

    for constraint in constraints {
        match constraint {
            Constraint::Equality(var, value) => {
                let cmp_source = CmpSource::equals(var, value);
                sources.push(Box::new(cmp_source));
            }
            Constraint::Inequality(var, value) => {
                let cmp_source = CmpSource::not_equals(var, value);
                sources.push(Box::new(cmp_source));
            }
        }
    }

    // Create product zipper (AND of all constraints)
    let product = ProductZipper::new(sources);

    // Execute query
    let mut results = BindingsSet::empty();
    for path in product.iterate() {
        let bindings = extract_bindings_from_path(path)?;
        results.push(bindings);
    }

    results
}
```

### Symmetric Matching (Both Sides Have Variables)

**Challenge**: MeTTa allows variables on both sides; MORK patterns are typically one-sided.

**Solution**: Two-pass matching
```rust
pub fn symmetric_match(left: &Atom, right: &Atom) -> BindingsSet {
    // Pass 1: Match left pattern against right
    let bindings_lr = match_asymmetric(left, right);

    // Pass 2: Match right pattern against left with existing bindings
    let mut final_bindings = BindingsSet::empty();

    for bindings in bindings_lr.iter() {
        // Substitute known bindings in right
        let right_subst = substitute(right, bindings);

        // Match substituted right against left
        let bindings_rl = match_asymmetric(&right_subst, left);

        // Merge bindings
        for new_bindings in bindings_rl.iter() {
            if let Some(merged) = bindings.merge(new_bindings) {
                final_bindings.push(merged);
            }
        }
    }

    final_bindings
}
```

---

## Space Operations

### Space as PathMap

**Mapping**:
```rust
pub struct MorkSpace {
    btm: PathMap<()>,  // Byte-trie map
    symbol_table: Arc<SharedMapping>,
    grounded_registry: Arc<GroundedRegistry>,
}
```

### add-atom Implementation

**Algorithm**:
```rust
impl MorkSpace {
    pub fn add(&mut self, atom: &Atom) -> Result<(), SpaceError> {
        // 1. Encode atom to bytes
        let mut ctx = VariableContext::new();
        let bytes = encode_atom(atom, &mut ctx, &self.symbol_table, &self.grounded_registry)?;

        // 2. Insert into PathMap
        let mut wz = self.btm.write_zipper();
        wz.move_to_path(&bytes);
        wz.set_val(Some(()));  // Unit value (just presence)

        // 3. Trigger observers
        self.notify_observers(SpaceEvent::Add(atom.clone()));

        Ok(())
    }
}
```

**Batched Addition**:
```rust
pub fn add_batch(&mut self, atoms: Vec<Atom>) -> Result<(), SpaceError> {
    let mut batch_map = PathMap::new();

    for atom in atoms {
        let bytes = encode_atom(&atom, ...)?;
        batch_map.insert(bytes, ());
    }

    // Single join operation
    self.btm.write_zipper().join_into(&batch_map.read_zipper());

    Ok(())
}
```

### remove-atom Implementation

**Algorithm**:
```rust
impl MorkSpace {
    pub fn remove(&mut self, atom: &Atom) -> Result<bool, SpaceError> {
        // 1. Encode atom
        let bytes = encode_atom(atom, ...)?;

        // 2. Remove from PathMap
        let removed = self.btm.remove(&bytes).is_some();

        // 3. Notify observers if removed
        if removed {
            self.notify_observers(SpaceEvent::Remove(atom.clone()));
        }

        Ok(removed)
    }
}
```

**Batched Removal**:
```rust
pub fn remove_batch(&mut self, atoms: Vec<Atom>) -> Result<usize, SpaceError> {
    let mut removal_map = PathMap::new();

    for atom in atoms {
        let bytes = encode_atom(&atom, ...)?;
        removal_map.insert(bytes, ());
    }

    // Single subtract operation
    let status = self.btm.write_zipper()
        .subtract_into(&removal_map.read_zipper(), true);

    Ok(removal_map.val_count())
}
```

### match (Query) Implementation

**Algorithm**:
```rust
impl MorkSpace {
    pub fn query(&self, pattern: &Atom) -> Result<BindingsSet, SpaceError> {
        // 1. Convert pattern to MORK source
        let source = pattern_to_source(pattern)?;

        // 2. Execute query
        let zipper = source.create_zipper(&self.btm);

        // 3. Collect results
        let mut results = BindingsSet::empty();

        for path in zipper.iterate() {
            let bindings = path_to_bindings(path, pattern)?;
            results.push(bindings);
        }

        Ok(results)
    }
}
```

### Multi-Space Management

**Implementation**:
```rust
pub struct SpaceManager {
    spaces: HashMap<String, MorkSpace>,
    default_space: String,
}

impl SpaceManager {
    pub fn new_space(&mut self, name: String) -> &mut MorkSpace {
        self.spaces.entry(name).or_insert_with(MorkSpace::new)
    }

    pub fn get_space(&self, name: &str) -> Option<&MorkSpace> {
        self.spaces.get(name)
    }

    pub fn get_space_mut(&mut self, name: &str) -> Option<&mut MorkSpace> {
        self.spaces.get_mut(name)
    }
}
```

**Usage**:
```metta
!(= &facts (new-space))
!(= &rules (new-space))

!(add-atom &facts (parent Alice Bob))
!(add-atom &rules (= (ancestor $x $y) (parent $x $y)))
```

---

## Evaluation Engine

### Minimal MeTTa Operations in MORK

#### 1. eval Implementation

```rust
pub fn eval(atom: &Atom, space: &MorkSpace) -> Vec<(Atom, Bindings)> {
    // Check if grounded function
    if let Some(func) = atom.as_grounded_function() {
        return execute_grounded(func, atom.children());
    }

    // Query space for rewrite rules: (= <pattern> <template>)
    let query_pattern = expr!("=" atom var!("$result"));

    let matches = space.query(&query_pattern)?;

    let mut results = Vec::new();
    for bindings in matches.iter() {
        let result = bindings.resolve(&var!("$result"))?;
        results.push((result, bindings.clone()));
    }

    // No reduction: return atom unchanged
    if results.is_empty() {
        results.push((atom.clone(), Bindings::new()));
    }

    results
}
```

#### 2. chain Implementation

```rust
pub fn chain(
    atom: &Atom,
    var: &VariableAtom,
    template: &Atom,
    space: &MorkSpace
) -> Vec<(Atom, Bindings)> {
    // Evaluate first atom
    let results1 = eval(atom, space);

    let mut final_results = Vec::new();

    for (result1, bindings1) in results1 {
        // Substitute variable in template
        let mut combined_bindings = bindings1.clone();
        combined_bindings.add_var_binding(var.clone(), result1);

        let substituted = substitute(template, &combined_bindings);

        // Evaluate substituted template
        let results2 = eval(&substituted, space);

        for (result2, bindings2) in results2 {
            // Merge bindings
            let mut merged = combined_bindings.clone();
            for (v, val) in bindings2.iter() {
                merged.add_var_binding(v.clone(), val.clone());
            }

            final_results.push((result2, merged));
        }
    }

    final_results
}
```

#### 3. unify Implementation

```rust
pub fn unify(
    atom: &Atom,
    pattern: &Atom,
    then_branch: &Atom,
    else_branch: &Atom,
    space: &MorkSpace
) -> Vec<(Atom, Bindings)> {
    // Match atom against pattern
    let matches = match_atoms(atom, pattern);

    if matches.is_empty() {
        // No match: eval else branch
        eval(else_branch, space)
    } else {
        // Match succeeded: eval then branch with bindings
        let mut results = Vec::new();

        for bindings in matches.iter() {
            let then_subst = substitute(then_branch, bindings);
            let then_results = eval(&then_subst, space);

            for (result, new_bindings) in then_results {
                let merged = bindings.merge(&new_bindings);
                results.push((result, merged));
            }
        }

        results
    }
}
```

### Non-Deterministic Evaluation

**State Tracking**:
```rust
pub struct EvalState {
    branches: Vec<(Atom, Bindings)>,
    depth: usize,
    max_depth: usize,
}

impl EvalState {
    pub fn eval_step(&mut self, space: &MorkSpace) -> bool {
        if self.depth >= self.max_depth {
            return false;  // Depth limit reached
        }

        let mut new_branches = Vec::new();
        let mut any_reduced = false;

        for (atom, bindings) in self.branches.drain(..) {
            let results = eval(&atom, space);

            if results.len() == 1 && results[0].0 == atom {
                // No reduction
                new_branches.push((atom, bindings));
            } else {
                // Reduced
                any_reduced = true;
                new_branches.extend(results);
            }
        }

        self.branches = new_branches;
        self.depth += 1;

        any_reduced
    }

    pub fn eval_to_completion(&mut self, space: &MorkSpace) -> Vec<(Atom, Bindings)> {
        while self.eval_step(space) {
            // Continue until no more reductions
        }

        self.branches.clone()
    }
}
```

### Grounded Execution via WASMSink

**Integration**:
```rust
pub fn execute_grounded(func: &GroundedFunc, args: &[Atom]) -> Vec<(Atom, Bindings)> {
    match func.execute(args) {
        Ok(results) => {
            results.into_iter()
                .map(|r| (r, Bindings::new()))
                .collect()
        }
        Err(e) => {
            // Return Error atom
            vec![(Atom::gnd(ErrorAtom(e.to_string())), Bindings::new())]
        }
    }
}
```

**WASM Bridge** (if using WASMSink):
```rust
pub struct WASMBridge {
    runtime: WASMRuntime,
    function_table: HashMap<String, WASMFunctionHandle>,
}

impl WASMBridge {
    pub fn call(&mut self, func_name: &str, args: &[Atom]) -> Result<Vec<Atom>, ExecError> {
        let handle = self.function_table.get(func_name)
            .ok_or(ExecError::UnknownFunction)?;

        // Serialize args to WASM memory
        let args_bytes = serialize_atoms(args)?;

        // Call WASM function
        let result_bytes = self.runtime.call(handle, args_bytes)?;

        // Deserialize results from WASM memory
        deserialize_atoms(&result_bytes)
    }
}
```

---

## Implementation Roadmap

### Phase 1: Core Encoding (Weeks 1-2)

**Deliverables**:
```rust
// encode.rs
pub fn encode_atom(atom: &Atom) -> Result<Vec<u8>, EncodeError>;
pub fn decode_atom(bytes: &[u8]) -> Result<Atom, DecodeError>;

// symbol_table.rs
pub struct SymbolTable {
    shared_mapping: Arc<SharedMapping>,
}

// Tests
#[test]
fn test_encode_decode_roundtrip();
#[test]
fn test_variable_conversion();
```

**Success Criteria**:
- All atom types encode/decode correctly
- Roundtrip property holds: `decode(encode(x)) == x`
- Performance: >100K atoms/second

### Phase 2: Pattern Matching (Weeks 3-5)

**Deliverables**:
```rust
// pattern.rs
pub fn pattern_to_source(pattern: &Atom) -> BTMSource;
pub fn match_in_space(space: &PathMap, pattern: &Atom) -> BindingsSet;

// bindings.rs
pub fn mork_results_to_bindings(paths: Iterator, ctx: &PatternContext) -> BindingsSet;

// Tests
#[test]
fn test_simple_pattern_match();
#[test]
fn test_variable_binding();
#[test]
fn test_nested_patterns();
```

**Success Criteria**:
- Simple patterns match correctly
- Variable bindings extracted correctly
- Multi-constraint queries work

### Phase 3: Space Operations (Weeks 6-7)

**Deliverables**:
```rust
// space.rs
pub struct MorkSpace { ... }

impl MorkSpace {
    pub fn add(&mut self, atom: &Atom);
    pub fn remove(&mut self, atom: &Atom) -> bool;
    pub fn query(&self, pattern: &Atom) -> BindingsSet;
}

// Tests
#[test]
fn test_add_remove();
#[test]
fn test_query_results();
#[test]
fn test_multi_space();
```

**Success Criteria**:
- Add/remove operations work
- Queries return correct results
- Multiple spaces independent

### Phase 4: Evaluation Engine (Weeks 8-10)

**Deliverables**:
```rust
// eval.rs
pub fn eval(atom: &Atom, space: &MorkSpace) -> Vec<(Atom, Bindings)>;
pub fn chain(atom: &Atom, var: &VariableAtom, template: &Atom, space: &MorkSpace) -> Vec<(Atom, Bindings)>;
pub fn unify(atom: &Atom, pattern: &Atom, then: &Atom, else: &Atom, space: &MorkSpace) -> Vec<(Atom, Bindings)>;

// Tests
#[test]
fn test_eval_rewrite();
#[test]
fn test_chain_composition();
#[test]
fn test_unify_branching();
```

**Success Criteria**:
- Minimal operations implemented
- Non-determinism handled correctly
- Evaluation terminates properly

### Phase 5-8: Advanced Features (Weeks 11-20)

**Grounded Atoms** (Weeks 11-12):
- Registry implementation
- Standard types (Number, String, Function)
- Custom type support

**Type System** (Weeks 13-14):
- Type checking via pattern matching
- Type inference integration
- Error reporting

**Module System** (Weeks 15-16):
- Module spaces
- Import/export
- Namespace handling

**Optimization** (Weeks 17-20):
- Profile critical paths
- Implement optimizations
- Benchmark suite
- Performance tuning

---

## Common Challenges & Solutions

### Challenge 1: Variable Scoping

**Problem**: De Bruijn levels shift with nesting.

**Solution**: Track scope depth, adjust levels on entry/exit
```rust
pub struct ScopeTracker {
    current_depth: usize,
}

impl ScopeTracker {
    pub fn enter_scope(&mut self) {
        self.current_depth += 1;
    }

    pub fn exit_scope(&mut self) {
        self.current_depth -= 1;
    }

    pub fn adjust_level(&self, level: u8) -> u8 {
        level + self.current_depth as u8
    }
}
```

### Challenge 2: Large Expression Encoding

**Problem**: Expressions > 239 children exceed arity tag.

**Solution**: Use nested expressions or extended encoding
```rust
fn encode_large_expression(children: &[Atom]) -> Vec<u8> {
    if children.len() <= 239 {
        // Standard encoding
        return encode_expression_standard(children);
    }

    // Split into chunks of 239
    let mut result = Vec::new();
    for chunk in children.chunks(239) {
        result.extend(encode_expression_standard(chunk));
    }
    result
}
```

### Challenge 3: Circular Variable References

**Problem**: `$x = $y, $y = $x`

**Solution**: Detect cycles during resolution
```rust
fn resolve_with_cycle_detection(bindings: &Bindings, var: &VariableAtom) -> Option<Atom> {
    let mut visited = HashSet::new();
    let mut current = var;

    loop {
        if visited.contains(current) {
            return None;  // Cycle detected
        }
        visited.insert(current.clone());

        match bindings.get(current) {
            Some(Binding::Atom(atom)) => return Some(atom.clone()),
            Some(Binding::Var(next_var)) => current = next_var,
            None => return None,
        }
    }
}
```

### Challenge 4: Grounded Atom Serialization

**Problem**: Not all Rust types are serializable.

**Solution**: Type-specific adapters with fallback to name references
```rust
pub enum GroundedEncoding {
    Serialized(Vec<u8>),
    NameReference(String),
    Handle(u64),
}

pub trait GroundedAdapter {
    fn encode(&self, atom: &dyn GroundedAtom) -> GroundedEncoding {
        // Try serialization first
        if let Ok(bytes) = self.try_serialize(atom) {
            return GroundedEncoding::Serialized(bytes);
        }

        // Fall back to name reference
        GroundedEncoding::NameReference(atom.type_().to_string())
    }
}
```

### Challenge 5: Performance of Repeated Encoding

**Problem**: Encoding same atoms repeatedly is wasteful.

**Solution**: Encoding cache
```rust
pub struct EncodingCache {
    cache: HashMap<Atom, Vec<u8>>,
    max_size: usize,
}

impl EncodingCache {
    pub fn encode_cached(&mut self, atom: &Atom) -> Vec<u8> {
        if let Some(cached) = self.cache.get(atom) {
            return cached.clone();
        }

        let encoded = encode_atom(atom);

        if self.cache.len() < self.max_size {
            self.cache.insert(atom.clone(), encoded.clone());
        }

        encoded
    }
}
```

---

## Testing Strategy

### Unit Tests

```rust
// Test atom encoding
#[test]
fn test_encode_symbol() {
    let sym = Atom::Symbol(SymbolAtom::new("foo"));
    let encoded = encode_atom(&sym).unwrap();
    let decoded = decode_atom(&encoded).unwrap();
    assert_eq!(sym, decoded);
}

// Test pattern matching
#[test]
fn test_simple_pattern() {
    let space = MorkSpace::new();
    space.add(&expr!("parent" "Alice" "Bob"));

    let pattern = expr!("parent" x y);
    let results = space.query(&pattern).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].resolve(&x), Some(sym!("Alice")));
}

// Test evaluation
#[test]
fn test_eval_rewrite() {
    let space = MorkSpace::new();
    space.add(&expr!("=" (expr!("double" x)) (expr!("+" x x))));

    let result = eval(&expr!("double" 5), &space);
    assert_eq!(result[0].0, expr!("+" 5 5));
}
```

### Integration Tests

```rust
#[test]
fn test_factorial() {
    let space = setup_factorial_space();
    let result = eval(&expr!("factorial" 5), &space);
    assert_eq!(result[0].0, Atom::gnd(Number::Integer(120)));
}

fn setup_factorial_space() -> MorkSpace {
    let space = MorkSpace::new();

    // Base case
    space.add(&expr!("=" (expr!("factorial" 0)) 1));

    // Recursive case
    space.add(&expr!("="
        (expr!("factorial" n))
        (expr!("*" n (expr!("factorial" (expr!("-" n 1))))));

    space
}
```

### Property-Based Tests (QuickCheck)

```rust
use quickcheck::{quickcheck, Arbitrary};

#[quickcheck]
fn prop_encode_decode_roundtrip(atom: Atom) -> bool {
    let encoded = encode_atom(&atom).unwrap();
    let decoded = decode_atom(&encoded).unwrap();
    atom == decoded
}

#[quickcheck]
fn prop_alpha_equivalence(expr1: Expr, expr2: Expr) -> bool {
    if are_alpha_equivalent(&expr1, &expr2) {
        let enc1 = encode_with_debruijn(&expr1);
        let enc2 = encode_with_debruijn(&expr2);
        enc1 == enc2
    } else {
        true
    }
}
```

---

## Performance Targets

**Encoding/Decoding**:
- Simple atoms: <100 ns
- Complex expressions: <1 µs
- Throughput: >1M atoms/second

**Pattern Matching**:
- Simple patterns: <10 µs
- Complex patterns: <100 µs
- Large spaces (10K atoms): <1 ms per query

**Evaluation**:
- Single eval step: <100 µs
- Complete evaluation (depth 10): <1 ms
- Grounded function call: <10 µs

**Memory**:
- Encoding overhead: <2× atom size
- Pattern matching: O(pattern size)
- Space storage: With prefix sharing, 10-100× compression

---

## Next Steps

1. **Start with Phase 1**: Implement core encoding/decoding
2. **Validate Early**: Test roundtrip property for all atom types
3. **Incremental Development**: Each phase builds on previous
4. **Continuous Testing**: Maintain test suite throughout
5. **Profile Regularly**: Identify bottlenecks early
6. **Document Decisions**: Keep implementation journal

**Success Metrics**:
- All MeTTa test cases pass
- Performance targets met
- Code coverage >80%
- Documentation complete

---

## Conclusion

This guide provides a complete implementation strategy for MeTTa-on-MORK:

✅ **Pattern Matching**: Hybrid variable representation, MORK source conversion
✅ **Space Operations**: PathMap-based storage, batch operations
✅ **Evaluation Engine**: Minimal operations, non-determinism handling
✅ **Roadmap**: 20-week phased implementation
✅ **Challenges**: Common problems with solutions

**Ready to implement**: All key algorithms and data structures specified.

**Reference Documents**:
- `metta-overview.md`: MeTTa language semantics
- `encoding-strategy.md`: Byte-level atom encoding
- This document: Implementation guide

Start with Phase 1 and proceed incrementally!
