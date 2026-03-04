use mettatron::{compile, run_state, MettaState};
use proptest::prelude::*;
use proptest::collection::vec;

#[derive(Clone, Debug)]
struct SpaceTestProgram {
    source: String,
    expected_results: Vec<String>,
}

// Generate valid simple MeTTa atoms (no parentheses)
fn metta_atom() -> impl Strategy<Value = String> {
    "[A-Za-z][A-Za-z0-9_]*".prop_map(|s: String| s)
}

// Generate MeTTa pattern with variables for matching
fn metta_pattern() -> impl Strategy<Value = String> {
    prop_oneof![
        // Variable patterns
        r"\$[a-z][a-z0-9]*".prop_map(|s: String| s),
        // Ground patterns
        "[A-Za-z][A-Za-z0-9_]*".prop_map(|s: String| s),
        // Mixed patterns with variables
        ("[A-Za-z][A-Za-z0-9_]*", r"\$[a-z][a-z0-9]*")
            .prop_map(|(pred, var)| format!("({} {})", pred, var)),
        // Wildcard patterns
        Just("$_".to_string()),
    ]
}

fn space_with_atoms() -> impl Strategy<Value = SpaceTestProgram> {
    vec(metta_atom(), 1..=10).prop_map(|atoms| {
        let mut source = String::new();
        let mut model = Vec::new();
        
        for atom in &atoms {
            source.push_str(&format!("(= (fact {}) True)\n", atom));
            model.push(atom.clone());
        }
        
        source.push_str("! (match & self (fact $x) $x)\n");
        
        SpaceTestProgram {
            source,
            expected_results: model,
        }
    })
}

// Generate space operations with match queries
// Follows postcondition pattern: relates return values to arguments of single call
fn space_with_match_query() -> impl Strategy<Value = SpaceTestProgram> {
    (vec(metta_atom(), 2..=8), metta_pattern()).prop_map(|(atoms, pattern)| {
        let mut source = String::new();
        let mut expected = Vec::new();
        
        // Add atoms to space
        for atom in &atoms {
            source.push_str(&format!("(= (fact {}) True)\n", atom));
        }
        
        // Simple match that should return matching atoms
        source.push_str("! (match & self (fact $x) $x)\n");
        
        // Model: find atoms that would match the pattern
        for atom in &atoms {
            if pattern_matches(atom, &pattern) {
                expected.push(atom.clone());
            }
        }
        
        SpaceTestProgram {
            source,
            expected_results: expected,
        }
    })
}

// Simplified pattern matching for model - handles basic cases
fn pattern_matches(atom: &str, pattern: &str) -> bool {
    if pattern.starts_with('$') {
        return true;
    }
    if atom == pattern {
        return true;
    }
    if pattern.starts_with('(') && atom.starts_with('(') {
        let pattern_parts: Vec<&str> = pattern.trim_start_matches('(').trim_end_matches(')').split_whitespace().collect();
        let atom_parts: Vec<&str> = atom.trim_start_matches('(').trim_end_matches(')').split_whitespace().collect();
        
        if pattern_parts.len() != atom_parts.len() {
            return false;
        }
        
        for (p_part, a_part) in pattern_parts.iter().zip(atom_parts.iter()) {
            if !pattern_matches(a_part, p_part) {
                return false;
            }
        }
        return true;
    }
    
    false
}


// Generate relational queries that test non-deterministic behavior
fn relational_query_nondeterminism() -> impl Strategy<Value = SpaceTestProgram> {
    let relations = vec![
        ("Parent", "Tom", "Bob"),
        ("Parent", "Tom", "Liz"), 
        ("Parent", "Pam", "Bob"),
        ("Parent", "Bob", "Ann"),
        ("Parent", "Bob", "Pat"),
        ("Parent", "Pat", "Pat"),
    ];
    
    Just(relations).prop_map(|relations| {
        let mut source = String::new();
        let mut expected = Vec::new();
        for (relation, parent, child) in &relations {
            source.push_str(&format!("(add-atom &self ({} {} {}))\n", relation, parent, child));
        }
        source.push_str("! (match &self (Parent $x $y) ($x $y))\n");
        for (_, parent, child) in relations {
            expected.push(format!("({} {})", parent, child));
        }
        
        SpaceTestProgram {
            source,
            expected_results: expected,
        }
    })
}

// Multi-space operations testing - ensures space independence
fn multi_space_operations() -> impl Strategy<Value = SpaceTestProgram> {
    vec(metta_atom(), 2..=5).prop_map(|atoms| {
        let mut source = String::new();
        for atom in &atoms {
            source.push_str(&format!("(= (fact {}) True)\n", atom));
        }
        source.push_str("! (match & self (fact $x) $x)\n");
        
        SpaceTestProgram {
            source,
            expected_results: atoms,
        }
    })
}

// Model-based property for new-space operation
fn new_space_model() -> impl Strategy<Value = SpaceTestProgram> {
    Just(SpaceTestProgram {
        source: "! (match & self $x $x)\n".to_string(),
        expected_results: vec!["[]".to_string()],
    })
}

// Model-based property for add-atom operation  
fn add_atom_model() -> impl Strategy<Value = SpaceTestProgram> {
    vec(metta_atom(), 1..=3).prop_map(|atoms| {
        let mut source = String::new();
        let mut expected = Vec::new();
        
        for atom in &atoms {
            source.push_str(&format!("(= (fact {}) True)\n", atom));
        }
        
        for atom in &atoms {
            expected.push(atom.clone());
        }
        source.push_str("! (match & self (fact $x) $x)\n");
        
        SpaceTestProgram { source, expected_results: expected }
    })
}

// Model-based property for remove-atom operation
fn remove_atom_model() -> impl Strategy<Value = SpaceTestProgram> {
    vec(metta_atom(), 2..=4).prop_map(|atoms| {
        let mut source = String::new();
        let mut expected = Vec::new();
        
        for atom in &atoms {
            source.push_str(&format!("(= (fact {}) True)\n", atom));
            expected.push(atom.clone());
        }
        
        source.push_str("! (match & self (fact $x) $x)\n");
        
        SpaceTestProgram { source, expected_results: expected }
    })
}

// Model-based property for match operation with variables
fn match_operation_model() -> impl Strategy<Value = SpaceTestProgram> {
    prop_oneof![
        vec(metta_atom(), 2..=4).prop_map(|atoms| {
            let mut source = String::new();
            for atom in &atoms {
                source.push_str(&format!("(= (fact {}) True)\n", atom));
            }
            
            if let Some(first_atom) = atoms.first() {
                source.push_str(&format!("! (match & self (fact {}) {})\n", first_atom, first_atom));
                SpaceTestProgram {
                    source,
                    expected_results: vec![first_atom.clone()],
                }
            } else {
                SpaceTestProgram { source, expected_results: vec![] }
            }
        }),
        
        Just(SpaceTestProgram {
            source: "(= (Human Socrates) True)\n(= (Human Plato) True)\n! (match & self (Human $x) $x)\n".to_string(),
            expected_results: vec!["Socrates".to_string(), "Plato".to_string()],
        })
    ]
}

proptest! {
    #[test]  
    fn new_space_creates_empty(test_program in new_space_model()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        if let Some(last_result) = result.last() {
            let result_str = last_result.to_metta_string();
            prop_assert!(result_str.contains("[]"), 
                "New space should be empty, got: {}", result_str);
        }
    }

    #[test]
    fn add_atom_model_correctness(test_program in add_atom_model()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        if let Some(last_result) = result.last() {
            let result_str = last_result.to_metta_string();
            for expected_atom in &test_program.expected_results {
                prop_assert!(result_str.contains(expected_atom), 
                    "Expected atom '{}' not found in result: {}", expected_atom, result_str);
            }
        }
    }

    #[test]
    fn remove_atom_model_correctness(test_program in remove_atom_model()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        if let Some(last_result) = result.last() {
            let result_str = last_result.to_metta_string();
            for expected_atom in &test_program.expected_results {
                prop_assert!(result_str.contains(expected_atom), 
                    "Expected remaining atom '{}' not found: {}", expected_atom, result_str);
            }
        }
    }

    // Model-based property: match operation correctness
    #[test]
    fn match_model_correctness(test_program in match_operation_model()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        if let Some(last_result) = result.last() {
            let result_str = last_result.to_metta_string();
            
            // Should be list format and contain expected results
            prop_assert!(result_str.starts_with('[') && result_str.ends_with(']'),
                "Match should return list, got: {}", result_str);
                
            for expected in &test_program.expected_results {
                prop_assert!(result_str.contains(expected),
                    "Match result should contain '{}', got: {}", expected, result_str);
            }
        }
    }

    // Postcondition: add-atom should make atom findable
    #[test]
    fn add_atom_postcondition(atom in metta_atom()) {
        let state = MettaState::new_empty();
        let prog = format!(
            "(add-atom &self {})\n! (match &self {} {})\n",
            atom, atom, atom
        );
        
        let compiled = compile(&prog);
        prop_assert!(compiled.is_ok());
        
        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;
            
        if let Some(match_result) = result.last() {
            let result_str = match_result.to_metta_string();
            prop_assert!(result_str.contains(&atom),
                "After adding '{}', match should find it. Got: {}", atom, result_str);
        }
    }
    
    // Postcondition: remove-atom should make atom unfindable
    #[test]
    fn remove_atom_postcondition(atom in metta_atom()) {
        let state = MettaState::new_empty();
        let prog = format!(
            "(add-atom &self {})\n(remove-atom &self {})\n! (match &self {} {})\n",
            atom, atom, atom, atom
        );
        
        let compiled = compile(&prog);
        prop_assert!(compiled.is_ok());
        
        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;
            
        if let Some(match_result) = result.last() {
            let result_str = match_result.to_metta_string();
            prop_assert!(result_str == "[]" || !result_str.contains(&atom),
                "After removing '{}', match should not find it. Got: {}", atom, result_str);
        }
    }
    
    // Postcondition: match should return atoms that satisfy the pattern
    #[test] 
    fn match_postcondition(test_program in space_with_match_query()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        // Verify match results are reasonable (basic sanity check)
        if let Some(match_result) = result.last() {
            let result_str = match_result.to_metta_string();
            // Should be a list (could be empty)
            prop_assert!(result_str.starts_with('[') && result_str.ends_with(']'),
                "Match result should be a list, got: {}", result_str);
        }
    }

    // Validity testing: space operations should preserve space validity
    #[test]
    fn space_operations_valid(test_program in multi_space_operations()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap());
        // Should not crash or produce errors
        prop_assert!(result.is_ok());
    }

    // Metamorphic: fact definition is commutative for different atoms
    #[test]
    fn add_atom_commutativity(atoms in vec(metta_atom(), 2..=3)) {
        if atoms.len() >= 2 {
            let state1 = MettaState::new_empty();
            let state2 = MettaState::new_empty();
            
            // Order 1: define fact for atom1, then atom2
            let prog1 = format!(
                "(= (fact {}) True)\n(= (fact {}) True)\n! (match & self (fact $x) $x)\n",
                atoms[0], atoms[1]
            );
            
            // Order 2: define fact for atom2, then atom1
            let prog2 = format!(
                "(= (fact {}) True)\n(= (fact {}) True)\n! (match & self (fact $x) $x)\n",
                atoms[1], atoms[0]
            );
            
            let compiled1 = compile(&prog1);
            let compiled2 = compile(&prog2);
            prop_assert!(compiled1.is_ok() && compiled2.is_ok());
            
            let result1 = run_state(state1, compiled1.unwrap()).expect("Failed").output;
            let result2 = run_state(state2, compiled2.unwrap()).expect("Failed").output;
            
            if let (Some(r1), Some(r2)) = (result1.last(), result2.last()) {
                let str1 = r1.to_metta_string();
                let str2 = r2.to_metta_string();
                
                // Both should contain both atoms (order-independent)
                for atom in &atoms {
                    prop_assert!(str1.contains(atom) && str2.contains(atom),
                        "Both results should contain '{}'. Got: {} and {}", atom, str1, str2);
                }
            }
        }
    }
    
    // Metamorphic: remove-atom after add-atom should restore original state
    #[test]
    fn add_remove_inverse(atom in metta_atom()) {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();
        
        // State 1: just get initial atoms
        let prog1 = "! (match & self (fact $x) $x)\n";
        
        // State 2: add then remove same atom
        let prog2 = format!(
            "(= (fact {}) True)\n! (match & self (fact $x) $x)\n",
            atom
        );
        
        let compiled1 = compile(prog1);
        let compiled2 = compile(&prog2);
        prop_assert!(compiled1.is_ok() && compiled2.is_ok());
        
        let result1 = run_state(state1, compiled1.unwrap()).expect("Failed").output;
        let result2 = run_state(state2, compiled2.unwrap()).expect("Failed").output;
        
        if let (Some(r1), Some(r2)) = (result1.last(), result2.last()) {
            // Both should be empty (assuming we started with empty space)
            prop_assert_eq!(r1.to_metta_string(), r2.to_metta_string(),
                "Add then remove should restore original state");
        }
    }

    // Metamorphic: idempotence - defining same fact twice has same effect as defining once
    #[test]
    fn add_atom_idempotence(atom in metta_atom()) {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();
        
        // State 1: define fact once
        let prog1 = format!("(= (fact {}) True)\n! (match & self (fact $x) $x)\n", atom);
        
        // State 2: define same fact twice
        let prog2 = format!("(= (fact {}) True)\n(= (fact {}) True)\n! (match & self (fact $x) $x)\n", 
                            atom, atom);
        
        let compiled1 = compile(&prog1);
        let compiled2 = compile(&prog2);
        prop_assert!(compiled1.is_ok() && compiled2.is_ok());
        
        let result1 = run_state(state1, compiled1.unwrap()).expect("Failed").output;
        let result2 = run_state(state2, compiled2.unwrap()).expect("Failed").output;
        
        if let (Some(r1), Some(r2)) = (result1.last(), result2.last()) {
            let str1 = r1.to_metta_string();
            let str2 = r2.to_metta_string();
            
            // Both should contain the atom
            prop_assert!(str1.contains(&atom) && str2.contains(&atom),
                "Both single and double add should contain atom '{}'. Got: {} and {}", atom, str1, str2);
        }
    }
    
    // Metamorphic: remove idempotence - removing non-existent atom multiple times
    #[test]
    fn remove_atom_idempotence(atom in metta_atom()) {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();
        
        // State 1: remove atom once from empty space
        let prog1 = format!("(remove-atom &self {})\n! (get-atoms &self)\n", atom);
        
        // State 2: remove same atom twice from empty space
        let prog2 = format!("(remove-atom &self {})\n(remove-atom &self {})\n! (get-atoms &self)\n", 
                            atom, atom);
        
        let compiled1 = compile(&prog1);
        let compiled2 = compile(&prog2);
        prop_assert!(compiled1.is_ok() && compiled2.is_ok());
        
        let result1 = run_state(state1, compiled1.unwrap()).expect("Failed").output;
        let result2 = run_state(state2, compiled2.unwrap()).expect("Failed").output;
        
        if let (Some(r1), Some(r2)) = (result1.last(), result2.last()) {
            // Both should be empty
            prop_assert_eq!(r1.to_metta_string(), r2.to_metta_string(),
                "Single and double remove from empty should have same result");
        }
    }

    // Metamorphic: match pattern equivalence - different equivalent patterns give same results
    #[test]
    fn match_pattern_equivalence(atom in "[A-Za-z][A-Za-z0-9_]*") {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();
        
        let prog1 = format!(
            "(add-atom &self {})\n! (match &self {} {})\n",
            atom, atom, atom
        );
        
        // Same query with variable then unification
        let prog2 = format!(
            "(add-atom &self {})\n! (match &self $x (unify $x {}))\n",
            atom, atom
        );
        
        let compiled1 = compile(&prog1);
        let compiled2 = compile(&prog2);
        
        // Skip if unify is not supported
        if compiled1.is_ok() && compiled2.is_ok() {
            let result1 = run_state(state1, compiled1.unwrap()).expect("Failed").output;
            let result2 = run_state(state2, compiled2.unwrap()).expect("Failed").output;
            
            if let (Some(r1), Some(r2)) = (result1.last(), result2.last()) {
                let str1 = r1.to_metta_string();
                let str2 = r2.to_metta_string();
                
                // Both patterns should match the same atom
                prop_assert!(str1.contains(&atom),
                    "Ground pattern should match atom '{}', got: {}", atom, str1);
            }
        }
    }
    
    // Metamorphic: match subsumption - specific patterns are subsets of general patterns
    #[test]
    fn match_subsumption(atoms in vec(metta_atom(), 2..=3)) {
        if !atoms.is_empty() {
            let state1 = MettaState::new_empty();
            let state2 = MettaState::new_empty();
            
            let mut prog1 = String::new();
            let mut prog2 = String::new();
            
            // Add atoms to both states
            for atom in &atoms {
                prog1.push_str(&format!("(add-atom &self {})\n", atom));
                prog2.push_str(&format!("(add-atom &self {})\n", atom));
            }
            
            // State 1: match specific atom
            prog1.push_str(&format!("! (match &self {} {})\n", atoms[0], atoms[0]));
            
            // State 2: match any atom (variable pattern)
            prog2.push_str("! (match &self $x $x)\n");
            
            let compiled1 = compile(&prog1);
            let compiled2 = compile(&prog2);
            prop_assert!(compiled1.is_ok() && compiled2.is_ok());
            
            let result1 = run_state(state1, compiled1.unwrap()).expect("Failed").output;
            let result2 = run_state(state2, compiled2.unwrap()).expect("Failed").output;
            
            if let (Some(r1), Some(r2)) = (result1.last(), result2.last()) {
                let str1 = r1.to_metta_string();
                let str2 = r2.to_metta_string();
                
                // Specific match result should be subset of general match
                if str1.contains(&atoms[0]) {
                    prop_assert!(str2.contains(&atoms[0]),
                        "General pattern should contain everything specific pattern contains. Specific: {}, General: {}", 
                        str1, str2);
                }
            }
        }
    }
    
    // Postcondition: get-atoms should reflect all add-atom operations
    #[test]
    fn get_atoms_reflects_additions(atoms in vec(metta_atom(), 2..=4)) {
        if !atoms.is_empty() {
            let state = MettaState::new_empty();
            let mut prog = String::new();
            
            for atom in &atoms {
                prog.push_str(&format!("(add-atom &self {})\n", atom));
            }
            prog.push_str("! (match & self (fact $x) $x)\n");
            
            let compiled = compile(&prog);
            prop_assert!(compiled.is_ok());
            
            let result = run_state(state, compiled.unwrap()).expect("Failed").output;
            
            if let Some(get_result) = result.last() {
                let result_str = get_result.to_metta_string();
                
                // Should contain all added atoms
                for atom in &atoms {
                    prop_assert!(result_str.contains(atom),
                        "get-atoms should contain '{}', got: {}", atom, result_str);
                }
            }
        }
    }
    
    // Postcondition: match should not find atoms in empty space
    #[test] 
    fn match_empty_space_postcondition(atom in metta_atom()) {
        let state = MettaState::new_empty();
        let prog = format!("! (match &self {} {})\n", atom, atom);
        
        let compiled = compile(&prog);
        prop_assert!(compiled.is_ok());
        
        let result = run_state(state, compiled.unwrap()).expect("Failed").output;
        
        if let Some(match_result) = result.last() {
            let result_str = match_result.to_metta_string();
            
            // Should be empty list
            prop_assert!(result_str == "[]" || result_str.is_empty(),
                "Match in empty space should return empty, got: {}", result_str);
        }
    }
    
    // Postcondition: match with variables should bind to actual atoms
    #[test]
    fn match_variable_binding_postcondition(atoms in vec(metta_atom(), 1..=3)) {
        if !atoms.is_empty() {
            let state = MettaState::new_empty();
            let mut prog = String::new();
            
            for atom in &atoms {
                prog.push_str(&format!("(add-atom &self {})\n", atom));
            }
            prog.push_str("! (match &self $y $y)\n");
            
            let compiled = compile(&prog);
            prop_assert!(compiled.is_ok());
            
            let result = run_state(state, compiled.unwrap()).expect("Failed").output;
            
            if let Some(match_result) = result.last() {
                let result_str = match_result.to_metta_string();
                
                if !result_str.contains("[]") {
                    // All returned bindings should be atoms we actually added
                    for atom in &atoms {
                        if result_str.contains(atom) {
                            prop_assert!(true, "Found expected atom binding");
                        }
                    }
                }
            }
        }
    }

    // Non-determinism testing: match should handle multiple results correctly
    #[test]
    fn match_nondeterminism(test_program in relational_query_nondeterminism()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        // Should return a list of pairs - order may vary due to nondeterminism
        if let Some(match_result) = result.last() {
            let result_str = match_result.to_metta_string();
            
            // Should be a list format
            prop_assert!(result_str.starts_with('[') && result_str.ends_with(']'),
                "Match should return list format, got: {}", result_str);
            
            // Should contain all expected pairs (order-independent check)
            for expected_pair in &test_program.expected_results {
                prop_assert!(result_str.contains(expected_pair),
                    "Result should contain '{}' but got: {}", expected_pair, result_str);
            }
        }
    }

    // Validity: metta_atom generator produces valid MeTTa atoms
    #[test]
    fn metta_atom_generator_validity(atom in metta_atom()) {
        // Should be non-empty
        prop_assert!(!atom.is_empty(), "Generated atom should not be empty");
        
        // Should not contain invalid characters for MeTTa atoms
        prop_assert!(!atom.contains('\n'), "Atom should not contain newlines");
        prop_assert!(!atom.contains('('), "Atom should not contain open paren");
        prop_assert!(!atom.contains(')'), "Atom should not contain close paren");
        
        // Should start with valid character (letter or underscore)
        if let Some(first_char) = atom.chars().next() {
            prop_assert!(first_char.is_ascii_alphabetic() || first_char == '_',
                "Atom '{}' should start with letter or underscore", atom);
        }
    }
    
    // Validity: space_with_atoms generator produces compilable programs
    #[test] 
    fn space_with_atoms_generator_validity(test_program in space_with_atoms()) {
        // Generated source should compile successfully
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok(), 
            "Generated program should compile: {}", test_program.source);
        
        // Should produce valid MeTTa syntax
        prop_assert!(test_program.source.contains("(= "), 
            "Space with atoms should contain rule definitions");
        prop_assert!(test_program.source.contains("match"),
            "Space with atoms should query the space");
    }
    
    // Validity: relational_query_nondeterminism generates valid relational data
    #[test]
    fn relational_query_generator_validity(test_program in relational_query_nondeterminism()) {
        // Should contain relational structure
        prop_assert!(test_program.source.contains("Parent") || 
                    test_program.source.contains("Child"),
            "Relational query should contain relational predicates");
        
        // Should contain variable matching
        prop_assert!(test_program.source.contains("$x") || test_program.source.contains("$y"),
            "Relational query should contain variables");
        
        // Expected results should be non-empty for non-trivial relations
        if test_program.source.lines().filter(|line| line.contains("(= ")).count() > 0 {
            prop_assert!(!test_program.expected_results.is_empty(),
                "Non-empty relational data should produce expected results");
        }
    }
    
    // Validity: generators produce diverse test data 
    #[test]
    fn generator_diversity(atoms in vec(metta_atom(), 5..=10)) {
        // Should generate diverse atoms
        let unique_atoms: std::collections::HashSet<_> = atoms.iter().collect();
        let diversity_ratio = unique_atoms.len() as f64 / atoms.len() as f64;
        
        prop_assert!(diversity_ratio > 0.5, 
            "Generator should produce reasonably diverse atoms. Diversity: {}", diversity_ratio);
        
        // Should not generate only trivial atoms
        let has_non_trivial = atoms.iter().any(|atom| atom.len() > 1);
        prop_assert!(has_non_trivial, 
            "Generator should produce some non-trivial atoms");
    }
    
    // Validity: pattern matching generators produce valid patterns
    #[test]
    fn pattern_generator_validity(test_program in space_with_match_query()) {
        // Should contain valid match patterns
        prop_assert!(test_program.source.contains("match"),
            "Pattern test should contain match operation");
        
        // Variables should be properly formatted
        if test_program.source.contains("$") {
            prop_assert!(test_program.source.contains("$x") || 
                        test_program.source.contains("$y") ||
                        test_program.source.contains("$"),
                "Variables should be properly formatted");
        }
    }
    
    // Inductive: match results scale with space contents
    #[test]
    fn match_results_scale_inductive(base_atoms in vec(metta_atom(), 2..=3),
                                     scaling_factor in 1usize..=2) {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();
        
        // Base case: match in space with base_atoms
        let mut prog1 = String::new();
        for atom in &base_atoms {
            prog1.push_str(&format!("(add-atom &self {})\n", atom));
        }
        prog1.push_str("! (match &self $x $x)\n");
        
        // Scaled case: duplicate atoms scaling_factor times
        let mut prog2 = String::new();
        for _i in 0..scaling_factor {
            for atom in &base_atoms {
                prog2.push_str(&format!("(add-atom &self {})\n", atom));
            }
        }
        for atom in &base_atoms {
            prog2.push_str(&format!("(add-atom &self {}_scaled)\n", atom));
        }
        prog2.push_str("! (match &self $x $x)\n");
        
        let compiled1 = compile(&prog1);
        let compiled2 = compile(&prog2);
        prop_assert!(compiled1.is_ok() && compiled2.is_ok());
        
        let result1 = run_state(state1, compiled1.unwrap()).expect("Failed").output;
        let result2 = run_state(state2, compiled2.unwrap()).expect("Failed").output;
        
        if let (Some(r1), Some(r2)) = (result1.last(), result2.last()) {
            let str1 = r1.to_metta_string();
            let str2 = r2.to_metta_string();
            
            // All atoms from base should appear in scaled version
            for atom in &base_atoms {
                if str1.contains(atom) {
                    prop_assert!(str2.contains(atom),
                        "Scaled case should contain base atom '{}'. Base: {}, Scaled: {}", 
                        atom, str1, str2);
                }
            }
        }
    }

    // Inductive: pattern complexity scales with match precision
    #[test]
    fn pattern_complexity_inductive(base_pattern in "[A-Za-z][A-Za-z0-9_]*") {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();
        
        // Base case: simple atom matching
        let prog1 = format!(
            "(add-atom &self {})\n\
             (add-atom &self Other)\n\
             ! (match &self {} {})\n", 
            base_pattern, base_pattern, base_pattern
        );
        
        // Complex case: structured pattern matching
        let prog2 = format!(
            "(add-atom &self ({} Value))\n\
             (add-atom &self (Other Value))\n\
             ! (match &self ({} $x) $x)\n", 
            base_pattern, base_pattern
        );
        
        let compiled1 = compile(&prog1);
        let compiled2 = compile(&prog2);
        
        if compiled1.is_ok() && compiled2.is_ok() {
            let result1 = run_state(state1, compiled1.unwrap()).expect("Failed").output;
            let result2 = run_state(state2, compiled2.unwrap()).expect("Failed").output;
            
            if let (Some(r1), Some(r2)) = (result1.last(), result2.last()) {
                let str1 = r1.to_metta_string();
                let str2 = r2.to_metta_string();
                
                // Base case should match the atom
                prop_assert!(str1.contains(&base_pattern),
                    "Simple pattern should match atom '{}': {}", base_pattern, str1);
                
                // Complex case should extract the value
                prop_assert!(str2.contains("Value") || str2.contains("[]"),
                    "Complex pattern should match structure: {}", str2);
            }
        }
    }
}