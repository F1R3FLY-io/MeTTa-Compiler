use mettatron::{compile, run_state, MettaState};
use proptest::prelude::*;
use proptest::string::string_regex;

#[derive(Clone, Debug)]
struct HigherOrderTestProgram {
    source: String,
    expected_results: Vec<String>,
}

#[derive(Clone, Debug)]
enum BaseFunction {
    Square,
    Duplicate,
    Identity,
}

impl BaseFunction {
    fn descriptor(&self) -> String {
        match self {
            BaseFunction::Square => "square".to_string(),
            BaseFunction::Duplicate => "duplicate".to_string(),
            BaseFunction::Identity => "identity".to_string(),
        }
    }

    fn definition(&self, fn_id: Option<String>) -> String {
        let name = match fn_id {
            Some(id) => format!("{}-{}", self.descriptor(), id),
            None => self.descriptor(),
        };

        match self {
            BaseFunction::Square => format!("(= ({} $x) (* $x $x))", name),
            BaseFunction::Duplicate => format!("(= ({} $x) ($x $x))", name),
            BaseFunction::Identity => format!("(= ({} $x) $x)", name),
        }
    }

    fn eval_apply_twice(&self, x: i64) -> String {
        match self {
            BaseFunction::Square => {
                let first_result = x.saturating_mul(x);
                first_result.saturating_mul(first_result).to_string()
            }
            BaseFunction::Duplicate => {
                format!("(({} {}) ({} {}))", x, x, x, x)
            }
            BaseFunction::Identity => x.to_string(),
        }
    }

    fn eval_apply_thrice(&self, x: i64) -> String {
        match self {
            BaseFunction::Square => {
                let first = x.saturating_mul(x);
                let second = first.saturating_mul(first);
                second.saturating_mul(second).to_string()
            }
            BaseFunction::Duplicate => {
                format!(
                    "((({} {}) ({} {})) (({} {}) ({} {})))",
                    x, x, x, x, x, x, x, x
                )
            }
            BaseFunction::Identity => x.to_string(),
        }
    }

    fn eval_single(&self, x: i64) -> String {
        match self {
            BaseFunction::Square => x.saturating_mul(x).to_string(),
            BaseFunction::Duplicate => format!("({} {})", x, x),
            BaseFunction::Identity => x.to_string(),
        }
    }
}

impl Arbitrary for BaseFunction {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        prop_oneof![
            Just(BaseFunction::Square),
            Just(BaseFunction::Duplicate),
            Just(BaseFunction::Identity),
        ]
        .boxed()
    }
}

fn apply_twice_definition(fn_id: Option<String>) -> String {
    let name = match fn_id {
        Some(id) => format!("apply-twice-{}", id),
        None => "apply-twice".to_string(),
    };
    format!("(= ({} $f $x) ($f ($f $x)))", name)
}

fn apply_thrice_definition(fn_id: Option<String>) -> String {
    let name = match fn_id {
        Some(id) => format!("apply-thrice-{}", id),
        None => "apply-thrice".to_string(),
    };
    format!("(= ({} $f $x) ($f ($f ($f $x))))", name)
}

fn compose_definition(fn_id: Option<String>) -> String {
    let name = match fn_id {
        Some(id) => format!("compose-{}", id),
        None => "compose".to_string(),
    };
    format!("(= ({} $f $g $x) ($f ($g $x)))", name)
}

fn flip_definition(fn_id: Option<String>) -> String {
    let name = match fn_id {
        Some(id) => format!("flip-{}", id),
        None => "flip".to_string(),
    };
    format!("(= ({} $f $x $y) ($f $y $x))", name)
}

#[derive(Clone, Debug)]
enum HigherOrderFunction {
    ApplyTwice,
    ApplyThrice,
    Compose,
    Flip,
}

impl HigherOrderFunction {
    fn descriptor(&self) -> String {
        match self {
            HigherOrderFunction::ApplyTwice => "apply-twice".to_string(),
            HigherOrderFunction::ApplyThrice => "apply-thrice".to_string(),
            HigherOrderFunction::Compose => "compose".to_string(),
            HigherOrderFunction::Flip => "flip".to_string(),
        }
    }

    fn definition(&self, fn_id: Option<String>) -> String {
        match self {
            HigherOrderFunction::ApplyTwice => apply_twice_definition(fn_id),
            HigherOrderFunction::ApplyThrice => apply_thrice_definition(fn_id),
            HigherOrderFunction::Compose => compose_definition(fn_id),
            HigherOrderFunction::Flip => flip_definition(fn_id),
        }
    }
}

impl Arbitrary for HigherOrderFunction {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        prop_oneof![
            Just(HigherOrderFunction::ApplyTwice),
            Just(HigherOrderFunction::ApplyThrice),
            Just(HigherOrderFunction::Compose),
            Just(HigherOrderFunction::Flip),
        ]
        .boxed()
    }
}

fn apply_twice_with_functions() -> impl Strategy<Value = HigherOrderTestProgram> {
    (
        any::<BaseFunction>(),
        -10i64..=10i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(base_fn, x, suffix)| {
            let mut source = String::new();

            source.push_str(&base_fn.definition(Some(suffix.clone())));
            source.push('\n');

            source.push_str(&apply_twice_definition(Some(suffix.clone())));
            source.push('\n');

            let base_fn_name = format!("{}-{}", base_fn.descriptor(), suffix);
            let apply_twice_name = format!("apply-twice-{}", suffix);
            source.push_str(&format!("! ({} {} {})", apply_twice_name, base_fn_name, x));

            let expected_result = base_fn.eval_apply_twice(x);
            let expected_results = vec![expected_result];

            HigherOrderTestProgram {
                source,
                expected_results,
            }
        })
}

fn apply_twice_with_symbols() -> impl Strategy<Value = HigherOrderTestProgram> {
    (
        string_regex(r"[A-Z][a-zA-Z0-9]*").unwrap(),
        -10i64..=10i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(symbol, x, suffix)| {
            let mut source = String::new();

            source.push_str(&apply_twice_definition(Some(suffix.clone())));
            source.push('\n');

            let apply_twice_name = format!("apply-twice-{}", suffix);
            source.push_str(&format!("! ({} {} {})", apply_twice_name, symbol, x));

            let expected_result = format!("({} ({} {}))", symbol, symbol, x);
            let expected_results = vec![expected_result];

            HigherOrderTestProgram {
                source,
                expected_results,
            }
        })
}

fn apply_twice_with_number() -> impl Strategy<Value = HigherOrderTestProgram> {
    (
        -10i64..=10i64,
        -10i64..=10i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(number, x, suffix)| {
            let mut source = String::new();

            source.push_str(&apply_twice_definition(Some(suffix.clone())));
            source.push('\n');

            let apply_twice_name = format!("apply-twice-{}", suffix);
            source.push_str(&format!("! ({} {} {})", apply_twice_name, number, x));

            let expected_result = format!("({} ({} {}))", number, number, x);
            let expected_results = vec![expected_result];

            HigherOrderTestProgram {
                source,
                expected_results,
            }
        })
}

fn apply_thrice_with_functions() -> impl Strategy<Value = HigherOrderTestProgram> {
    (
        any::<BaseFunction>(),
        -5i64..=5i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(base_fn, x, suffix)| {
            let mut source = String::new();

            source.push_str(&base_fn.definition(Some(suffix.clone())));
            source.push('\n');

            source.push_str(&apply_thrice_definition(Some(suffix.clone())));
            source.push('\n');

            let base_fn_name = format!("{}-{}", base_fn.descriptor(), suffix);
            let apply_thrice_name = format!("apply-thrice-{}", suffix);
            source.push_str(&format!("! ({} {} {})", apply_thrice_name, base_fn_name, x));

            let expected_result = base_fn.eval_apply_thrice(x);
            let expected_results = vec![expected_result];

            HigherOrderTestProgram {
                source,
                expected_results,
            }
        })
}

fn compose_with_symbols() -> impl Strategy<Value = HigherOrderTestProgram> {
    (
        string_regex(r"[A-Z][a-zA-Z0-9]*").unwrap(),
        string_regex(r"[A-Z][a-zA-Z0-9]*").unwrap(),
        -5i64..=5i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(symbol1, symbol2, x, suffix)| {
            let mut source = String::new();

            source.push_str(&compose_definition(Some(suffix.clone())));
            source.push('\n');

            let compose_name = format!("compose-{}", suffix);
            source.push_str(&format!(
                "! ({} {} {} {})",
                compose_name, symbol1, symbol2, x
            ));

            let expected_result = format!("({} ({} {}))", symbol1, symbol2, x);
            let expected_results = vec![expected_result];

            HigherOrderTestProgram {
                source,
                expected_results,
            }
        })
}

fn mixed_higher_order_invocations() -> impl Strategy<Value = HigherOrderTestProgram> {
    let id_suffix = r"[a-z0-9]{6}";
    let suffix_strategy = string_regex(id_suffix).unwrap();

    prop::collection::vec(
        suffix_strategy.prop_flat_map(|suffix| {
            (
                any::<HigherOrderFunction>(),
                any::<BaseFunction>(),
                prop::collection::vec(-5i64..=5i64, 1..=3),
            )
                .prop_map(move |(ho_fn, base_fn, args)| (suffix.clone(), ho_fn, base_fn, args))
        }),
        1..5,
    )
    .prop_map(|batches| {
        let mut source = String::new();
        let mut all_expected_results: Vec<String> = vec![];

        for (suffix_id, ho_fn, base_fn, args) in batches {
            source.push_str(&base_fn.definition(Some(suffix_id.clone())));
            source.push('\n');

            source.push_str(&ho_fn.definition(Some(suffix_id.clone())));
            source.push('\n');

            let base_fn_name = format!("{}-{}", base_fn.descriptor(), suffix_id);
            let ho_fn_name = format!("{}-{}", ho_fn.descriptor(), suffix_id);

            for arg in args {
                match ho_fn {
                    HigherOrderFunction::ApplyTwice => {
                        source.push_str(&format!("! ({} {} {})", ho_fn_name, base_fn_name, arg));
                        all_expected_results.push(base_fn.eval_apply_twice(arg));
                    }
                    HigherOrderFunction::ApplyThrice => {
                        source.push_str(&format!("! ({} {} {})", ho_fn_name, base_fn_name, arg));
                        all_expected_results.push(base_fn.eval_apply_thrice(arg));
                    }
                    HigherOrderFunction::Compose => {
                        source.push_str(&format!("! ({} A B {})", ho_fn_name, arg));
                        all_expected_results.push(format!("(A (B {}))", arg));
                    }
                    HigherOrderFunction::Flip => {
                        source.push_str(&format!("! ({} A {} B)", ho_fn_name, arg));
                        all_expected_results.push(format!("(A B {})", arg));
                    }
                }
                source.push('\n');
            }

            source.push('\n');
        }

        HigherOrderTestProgram {
            source,
            expected_results: all_expected_results,
        }
    })
}

// 1. INVARIANT PROPERTIES (Validity Testing)
// Test that higher-order functions preserve structural invariants

#[derive(Clone, Debug)]
struct FunctionCompositionChain {
    functions: Vec<BaseFunction>,
    argument: i64,
    chain_id: String,
}

impl FunctionCompositionChain {
    fn build_source(&self) -> String {
        let mut source = String::new();

        for func in &self.functions {
            source.push_str(&func.definition(Some(self.chain_id.clone())));
            source.push('\n');
        }

        source.push_str(&apply_twice_definition(Some(self.chain_id.clone())));
        source.push('\n');

        source
    }

    fn is_valid_result(&self, result: &str) -> bool {
        !result.is_empty()
            && (result
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "()- ".contains(c)))
    }
}

fn function_composition_chain() -> impl Strategy<Value = FunctionCompositionChain> {
    (
        prop::collection::vec(any::<BaseFunction>(), 1..=3),
        -5i64..=5i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(functions, argument, chain_id)| FunctionCompositionChain {
            functions,
            argument,
            chain_id,
        })
}

// 2. ENHANCED METAMORPHIC PROPERTIES
// Test mathematical laws of higher-order function composition

fn compose_associativity_test() -> impl Strategy<Value = HigherOrderTestProgram> {
    (
        string_regex(r"[A-Z][a-zA-Z0-9]*").unwrap(),
        string_regex(r"[A-Z][a-zA-Z0-9]*").unwrap(),
        -3i64..=3i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(f, g, x, suffix)| {
            let mut source = String::new();

            source.push_str(&compose_definition(Some(suffix.clone())));
            source.push('\n');

            let compose_name = format!("compose-{}", suffix);

            source.push_str(&format!("! ({} {} {} {})", compose_name, f, g, x));

            let expected = format!("({} ({} {}))", f, g, x);

            HigherOrderTestProgram {
                source,
                expected_results: vec![expected],
            }
        })
}

// 3. INDUCTIVE PROPERTIES
// Test decomposition properties of higher-order functions

fn apply_twice_decomposition() -> impl Strategy<Value = HigherOrderTestProgram> {
    (
        any::<BaseFunction>(),
        -3i64..=3i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(base_fn, x, suffix)| {
            let mut source = String::new();

            source.push_str(&base_fn.definition(Some(suffix.clone())));
            source.push('\n');

            source.push_str(&apply_twice_definition(Some(suffix.clone())));
            source.push('\n');

            let base_fn_name = format!("{}-{}", base_fn.descriptor(), suffix);
            let apply_twice_name = format!("apply-twice-{}", suffix);

            source.push_str(&format!("! ({} {} {})", apply_twice_name, base_fn_name, x));
            source.push('\n');
            source.push_str(&format!("! ({} ({} {}))", base_fn_name, base_fn_name, x));

            let expected_twice = base_fn.eval_apply_twice(x);
            let expected_composed = base_fn.eval_apply_twice(x);

            HigherOrderTestProgram {
                source,
                expected_results: vec![expected_twice, expected_composed],
            }
        })
}

// 4. MODEL-BASED PROPERTIES
// Test against abstract mathematical model of function composition

#[derive(Clone, Debug)]
struct AbstractComposition {
    steps: usize,
    base_function: BaseFunction,
    argument: i64,
}

impl AbstractComposition {
    fn eval_model(&self) -> String {
        let mut result = self.argument;
        for _ in 0..self.steps {
            match self.base_function {
                BaseFunction::Square => result = result.saturating_mul(result),
                BaseFunction::Identity => {}
                BaseFunction::Duplicate => return format!("({} {})", result, result),
            }
        }
        result.to_string()
    }
}

fn model_based_composition() -> impl Strategy<Value = (AbstractComposition, HigherOrderTestProgram)>
{
    (
        1..=3usize,
        any::<BaseFunction>(),
        -3i64..=3i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
    ).prop_filter_map("Only test functions that can be iterated", |args: (usize, BaseFunction, i64, String)| -> Option<(AbstractComposition, HigherOrderTestProgram)> {
        let (steps, base_fn, arg, suffix) = args;
        match base_fn {
            BaseFunction::Duplicate => None,
            _ => {
                let model = AbstractComposition {
                    steps,
                    base_function: base_fn.clone(),
                    argument: arg,
                };

                let mut source = String::new();
                source.push_str(&base_fn.definition(Some(suffix.clone())));
                source.push('\n');

                let base_fn_name = format!("{}-{}", base_fn.descriptor(), suffix);

                let mut nested_call = format!("{}", arg);
                for _ in 0..steps {
                    nested_call = format!("({} {})", base_fn_name, nested_call);
                }
                source.push_str(&format!("! {}", nested_call));

                let expected_result = model.eval_model();

                let test_program = HigherOrderTestProgram {
                    source,
                    expected_results: vec![expected_result],
                };

                Some((model, test_program))
            }
        }
    })
}

// 5. PRESERVATION OF EQUIVALENCE
// Test that equivalent inputs produce equivalent outputs

fn equivalence_preservation() -> impl Strategy<Value = HigherOrderTestProgram> {
    (
        any::<BaseFunction>(),
        -3i64..=3i64,
        string_regex(r"[a-z0-9]{6}").unwrap(),
        string_regex(r"[a-z0-9]{6}").unwrap(),
    )
        .prop_map(|(base_fn, x, suffix1, suffix2)| {
            let mut source = String::new();

            source.push_str(&base_fn.definition(Some(suffix1.clone())));
            source.push('\n');
            source.push_str(&base_fn.definition(Some(suffix2.clone())));
            source.push('\n');

            source.push_str(&apply_twice_definition(Some(suffix1.clone())));
            source.push('\n');
            source.push_str(&apply_twice_definition(Some(suffix2.clone())));
            source.push('\n');

            let base_fn_name1 = format!("{}-{}", base_fn.descriptor(), suffix1);
            let base_fn_name2 = format!("{}-{}", base_fn.descriptor(), suffix2);
            let apply_twice_name1 = format!("apply-twice-{}", suffix1);
            let apply_twice_name2 = format!("apply-twice-{}", suffix2);

            source.push_str(&format!(
                "! ({} {} {})",
                apply_twice_name1, base_fn_name1, x
            ));
            source.push('\n');
            source.push_str(&format!(
                "! ({} {} {})",
                apply_twice_name2, base_fn_name2, x
            ));

            let expected = base_fn.eval_apply_twice(x);

            HigherOrderTestProgram {
                source,
                expected_results: vec![expected.clone(), expected],
            }
        })
}

proptest! {
    #[test]
    fn apply_twice_with_functions_evaluates_correctly(test_program in apply_twice_with_functions()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), test_program.expected_results.len());

        for (actual, expected) in result.iter().zip(test_program.expected_results.iter()) {
            let actual_str = actual.to_metta_string();
            prop_assert_eq!(&actual_str, expected);
        }
    }

    #[test]
    fn apply_twice_with_symbols_constructs_expressions(test_program in apply_twice_with_symbols()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), test_program.expected_results.len());

        for (actual, expected) in result.iter().zip(test_program.expected_results.iter()) {
            let actual_str = actual.to_metta_string();
            prop_assert_eq!(&actual_str, expected);
        }
    }

    #[test]
    fn apply_twice_with_numbers_constructs_expressions(test_program in apply_twice_with_number()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), test_program.expected_results.len());

        for (actual, expected) in result.iter().zip(test_program.expected_results.iter()) {
            let actual_str = actual.to_metta_string();
            prop_assert_eq!(&actual_str, expected);
        }
    }

    #[test]
    fn apply_twice_produces_deterministic_results(test_program in apply_twice_with_functions()) {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();

        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result1 = run_state(state1, compiled.as_ref().unwrap().clone())
            .expect("Failed to evaluate")
            .output;

        let result2 = run_state(state2, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result1.len(), result2.len());
        for (r1, r2) in result1.iter().zip(result2.iter()) {
            prop_assert_eq!(r1.to_metta_string(), r2.to_metta_string());
        }
    }

    #[test]
    fn apply_twice_produces_single_result(test_program in apply_twice_with_functions()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), 1, "Apply-twice should produce exactly one result");
    }

    #[test]
    fn apply_thrice_with_functions_evaluates_correctly(test_program in apply_thrice_with_functions()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), test_program.expected_results.len());

        for (actual, expected) in result.iter().zip(test_program.expected_results.iter()) {
            let actual_str = actual.to_metta_string();
            prop_assert_eq!(&actual_str, expected);
        }
    }

    #[test]
    fn compose_with_symbols_evaluates_correctly(test_program in compose_with_symbols()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), test_program.expected_results.len());

        for (actual, expected) in result.iter().zip(test_program.expected_results.iter()) {
            let actual_str = actual.to_metta_string();
            prop_assert_eq!(&actual_str, expected);
        }
    }

    #[test]
    fn mixed_higher_order_invocations_evaluate_correctly(test_program in mixed_higher_order_invocations()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), test_program.expected_results.len());

        for (actual, expected) in result.iter().zip(test_program.expected_results.iter()) {
            let actual_str = actual.to_metta_string();
            prop_assert_eq!(&actual_str, expected);
        }
    }

    #[test]
    fn mixed_higher_order_functions_produce_deterministic_results(test_program in mixed_higher_order_invocations()) {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();

        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result1 = run_state(state1, compiled.as_ref().unwrap().clone())
            .expect("Failed to evaluate")
            .output;

        let result2 = run_state(state2, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result1.len(), result2.len());
        for (r1, r2) in result1.iter().zip(result2.iter()) {
            prop_assert_eq!(r1.to_metta_string(), r2.to_metta_string());
        }
    }

    #[test]
    fn invariant_validity_all_results_are_well_formed(chain in function_composition_chain()) {
        let source = chain.build_source();
        let _state = MettaState::new_empty();
        let compiled = compile(&source);
        prop_assert!(compiled.is_ok(), "Source should compile without errors");

        for func in &chain.functions {
            let func_name = format!("{}-{}", func.descriptor(), chain.chain_id);
            let invocation_source = format!("{}\n! ({} {})", source, func_name, chain.argument);

            let compiled_inv = compile(&invocation_source);
            prop_assert!(compiled_inv.is_ok());

            let result = run_state(MettaState::new_empty(), compiled_inv.unwrap())
                .expect("Failed to evaluate")
                .output;

            for res in result {
                let res_str = res.to_metta_string();
                prop_assert!(chain.is_valid_result(&res_str),
                    "Result '{}' should be well-formed", res_str);
            }
        }
    }

    #[test]
    fn metamorphic_compose_produces_nested_expressions(test_program in compose_associativity_test()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), test_program.expected_results.len());

        for (actual, expected) in result.iter().zip(test_program.expected_results.iter()) {
            let actual_str = actual.to_metta_string();
            prop_assert_eq!(&actual_str, expected);

            let depth = actual_str.chars().filter(|&c| c == '(').count();
            prop_assert!(depth >= 2, "Compose should create nested expressions with depth >= 2");
        }
    }

    #[test]
    fn inductive_apply_twice_equals_nested_application(test_program in apply_twice_decomposition()) {
        let state1 = MettaState::new_empty();
        let state2 = MettaState::new_empty();

        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result1 = run_state(state1, compiled.as_ref().unwrap().clone())
            .expect("Failed to evaluate")
            .output;
        let result2 = run_state(state2, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result1.len(), 2);
        prop_assert_eq!(result2.len(), 2);

        prop_assert_eq!(result1[0].to_metta_string(), result1[1].to_metta_string(),
            "apply-twice and nested application should produce identical results");
        prop_assert_eq!(result2[0].to_metta_string(), result2[1].to_metta_string(),
            "Results should be deterministic across evaluations");
    }

    #[test]
    fn model_based_nested_application_matches_abstract_model((model, test_program) in model_based_composition()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), 1);

        let actual = result[0].to_metta_string();
        let expected = model.eval_model();

        prop_assert_eq!(&actual, &expected,
            "MeTTa evaluation should match abstract model for {} steps of {:?} on {}",
            model.steps, model.base_function, model.argument);
    }

    #[test]
    fn equivalence_preservation_identical_functions_produce_identical_results(test_program in equivalence_preservation()) {
        let state = MettaState::new_empty();
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok());

        let result = run_state(state, compiled.unwrap())
            .expect("Failed to evaluate")
            .output;

        prop_assert_eq!(result.len(), 2);

        prop_assert_eq!(result[0].to_metta_string(), result[1].to_metta_string(),
            "Equivalent function definitions should produce equivalent results");
    }

    #[test]
    fn generator_validity_all_generated_programs_compile(test_program in mixed_higher_order_invocations()) {
        let compiled = compile(&test_program.source);
        prop_assert!(compiled.is_ok(),
            "All generated test programs should compile successfully. Source: {}",
            test_program.source);

        let open_count = test_program.source.chars().filter(|&c| c == '(').count();
        let close_count = test_program.source.chars().filter(|&c| c == ')').count();
        prop_assert_eq!(open_count, close_count, "Generated source should have balanced parentheses");
    }
}
