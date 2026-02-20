use std::collections::HashMap;

use mettatron::{compile, run_state, MettaState, MettaValue};
use proptest::prelude::*;
use proptest::string::string_regex;

#[derive(Clone, Debug)]
struct FlatTestProgram {
    source: String,

    // oracle
    invocations: Vec<i64>,
}

#[derive(Clone, Debug)]
enum FnEquality {
    // (= (identity $x) $x)
    Identity,

    // (= (square $x) (* $x $x))
    Square,

    // (= (add $x $y) (+ $x $y))
    Add,

    // (= (subtract $x $y) (- $x $y))
    Subtract,

    // (= (multiply $x $y) (* $x $y))
    Multiply,
}

impl FnEquality {
    fn descriptor(&self) -> String {
        match self {
            FnEquality::Identity => "identity".to_string(),
            FnEquality::Square => "square".to_string(),
            FnEquality::Add => "add".to_string(),
            FnEquality::Subtract => "subtract".to_string(),
            FnEquality::Multiply => "multiply".to_string(),
        }
    }

    /// (= (sum $x $y) (+ $x $y))
    fn metta_fn_definition(&self, fn_id: Option<String>) -> String {
        match fn_id {
            Some(id) => {
                let descriptor_with_id = format!("{}-{}", self.descriptor(), id);
                format!(
                    "(= ({} {}) {})",
                    descriptor_with_id,
                    self.arguments(),
                    self.body()
                )
            }
            None => format!(
                "(= ({} {}) {})",
                self.descriptor(),
                self.arguments(),
                self.body()
            ),
        }
    }

    /// ! (sum 5 6)
    fn metta_invocation(&self, arguments: Vec<i64>, fn_id: Option<String>) -> String {
        let arguments = arguments
            .iter()
            .map(|x| x.to_string())
            .collect::<Vec<_>>()
            .join(" ");

        match fn_id {
            Some(id) => {
                let descriptor_with_id = format!("{}-{}", self.descriptor(), id);
                format!("! ({} {})", descriptor_with_id, arguments)
            }
            None => format!("! ({} {})", self.descriptor(), arguments),
        }
    }

    fn eval(&self, arguments: Vec<i64>) -> i64 {
        match self {
            FnEquality::Identity => arguments[0],
            FnEquality::Square => arguments[0].saturating_pow(2),
            FnEquality::Add => arguments[0].saturating_add(arguments[1]),
            FnEquality::Subtract => arguments[0].saturating_sub(arguments[1]),
            FnEquality::Multiply => arguments[0].saturating_mul(arguments[1]),
        }
    }

    fn arguments(&self) -> String {
        match self {
            FnEquality::Identity => "$x".to_string(),
            FnEquality::Square => "$x".to_string(),
            FnEquality::Add => "$x $y".to_string(),
            FnEquality::Subtract => "$x $y".to_string(),
            FnEquality::Multiply => "$x $y".to_string(),
        }
    }

    fn body(&self) -> String {
        match self {
            FnEquality::Identity => "$x".to_string(),
            FnEquality::Square => "(* $x $x)".to_string(),
            FnEquality::Add => "(+ $x $y)".to_string(),
            FnEquality::Subtract => "(- $x $y)".to_string(),
            FnEquality::Multiply => "(* $x $y)".to_string(),
        }
    }

    fn arity(&self) -> usize {
        match self {
            FnEquality::Identity => 1,
            FnEquality::Square => 1,
            FnEquality::Add => 2,
            FnEquality::Subtract => 2,
            FnEquality::Multiply => 2,
        }
    }
}

impl Arbitrary for FnEquality {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        prop_oneof![
            Just(FnEquality::Identity),
            Just(FnEquality::Square),
            Just(FnEquality::Add),
            Just(FnEquality::Subtract),
            Just(FnEquality::Multiply),
        ]
        .boxed()
    }
}

/// Output signature:
/// (fn_equality, invocations<invocation args>); example:
/// (FnEquality::Add, Vec[Vec[5, 2], Vec[3, 5], Vec[-2, 1]]
fn fn_with_invocations() -> impl Strategy<Value = (FnEquality, Vec<Vec<i64>>)> {
    any::<FnEquality>().prop_flat_map(|equality| {
        prop::collection::vec(
            // Arguments expected to be numbers
            prop::collection::vec(-10000i64..=10000i64, equality.arity()..=equality.arity()),
            1..10,
        )
        .prop_map(move |args| (equality.clone(), args))
    })
}

/// Generates program with unary fn and a bunch of it's onvocations:
/// (= (identity $x) $x)
/// ! (identity 5)
/// ! (identity 2)
/// ...
fn unary_invocations() -> impl Strategy<Value = FlatTestProgram> {
    fn_with_invocations().prop_map(|(equality, invocations)| {
        let mut res = String::new();
        res.push_str(&equality.metta_fn_definition(None));
        res.push('\n');

        let mut invocations_res: Vec<i64> = vec![];

        for invocation in invocations {
            res.push_str(&equality.metta_invocation(invocation.clone(), None));
            res.push('\n');

            invocations_res.push(equality.eval(invocation));
        }

        FlatTestProgram {
            source: res,
            invocations: invocations_res,
        }
    })
}

/// Generates program with multiple different functions and their invocations:
/// (= (identity-abc123 $x) $x)
/// ! (identity-abc123 5)
/// ! (identity-abc123 2)
/// ...
/// (= (square-xyz789 $x) (* $x $x))
/// ! (square-xyz789 5)
/// ! (square-xyz789 2)
/// ...
fn multiple_flat_invocations() -> impl Strategy<Value = FlatTestProgram> {
    let id_suffix = r"[a-z0-9]{6}";
    let suffix_strategy = string_regex(id_suffix).unwrap();

    prop::collection::vec(
        suffix_strategy.prop_flat_map(|suffix| {
            fn_with_invocations()
                .prop_map(move |(equality, invocations)| (suffix.clone(), equality, invocations))
        }),
        1..10,
    )
    .prop_map(|batches| {
        let mut source = String::new();
        let mut all_invocations_res: Vec<i64> = vec![];

        for (suffix_id, equality, invocations) in batches {
            source.push_str(&equality.metta_fn_definition(Some(suffix_id.clone())));
            source.push('\n');

            for invocation in invocations {
                source.push_str(
                    &equality.metta_invocation(invocation.clone(), Some(suffix_id.clone())),
                );
                source.push('\n');

                all_invocations_res.push(equality.eval(invocation));
            }

            source.push('\n');
        }

        FlatTestProgram {
            source,
            invocations: all_invocations_res,
        }
    })
}

#[derive(Clone, Debug)]
struct NestedTestProgram {
    source: String,

    // oracle
    reductions: Vec<i64>,
}

/// Represents an argument that can be either a literal or a function call
#[derive(Clone, Debug)]
enum RecursiveArg {
    Literal(i64),
    FunctionCall(String, Vec<RecursiveArg>),
}

impl RecursiveArg {
    fn to_metta_string(&self) -> String {
        match self {
            RecursiveArg::Literal(n) => n.to_string(),
            RecursiveArg::FunctionCall(fn_id, args) => {
                let args_str = args
                    .iter()
                    .map(|a| a.to_metta_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("({} {})", fn_id, args_str)
            }
        }
    }

    fn eval(&self, registry: &HashMap<String, FnEquality>) -> i64 {
        match self {
            RecursiveArg::Literal(n) => *n,
            RecursiveArg::FunctionCall(fn_id, args) => {
                let equality = registry.get(fn_id).expect("Function not in registry");
                let arg_values: Vec<i64> = args.iter().map(|a| a.eval(registry)).collect();
                equality.eval(arg_values)
            }
        }
    }
}

/// Generates recursive arguments using prop_recursive
fn recursive_arg_strategy(
    registry: Vec<(String, FnEquality)>,
) -> impl Strategy<Value = RecursiveArg> {
    // Range is kept small to prevent overflow at any nesting depth:
    // worst case sq(sq(sq(100))) = 10^16 < i64::MAX
    let leaf = prop_oneof![(-100i64..=100i64).prop_map(RecursiveArg::Literal),];

    leaf.prop_recursive(2, 64, 3, move |inner| {
        let reg = registry.clone();
        prop_oneof![
            prop::sample::select(reg.clone()).prop_flat_map(move |(fn_id, equality)| {
                let arity = equality.arity();
                prop::collection::vec(inner.clone(), arity..=arity)
                    .prop_map(move |args| RecursiveArg::FunctionCall(fn_id.clone(), args))
            }),
        ]
    })
}

/// Generates program with recursive function invocations:
/// (= (identity-abc123 $x) $x)
/// (= (sum-xyz789 $x $y) (+ $x $y))
/// ! (identity-abc123 5)
/// ! (identity-abc123 (sum-xyz789 2 3))
/// ! (identity-skz (sum-xyz789 (square-abc 2) 3))
fn nested_invocations() -> impl Strategy<Value = NestedTestProgram> {
    let id_suffix = r"[a-z0-9]{6}";

    // Generate function definitions
    prop::collection::vec(
        string_regex(id_suffix)
            .unwrap()
            .prop_flat_map(|suffix| any::<FnEquality>().prop_map(move |eq| (suffix.clone(), eq))),
        1..10,
    )
    .prop_flat_map(|function_defs| {
        let mut registry: HashMap<String, FnEquality> = HashMap::new();
        let mut registry_vec: Vec<(String, FnEquality)> = Vec::new();
        let mut source = String::new();

        for (suffix_id, equality) in &function_defs {
            let fn_id = format!("{}-{}", equality.descriptor(), suffix_id);
            registry.insert(fn_id.clone(), equality.clone());
            registry_vec.push((fn_id, equality.clone()));
            source.push_str(&equality.metta_fn_definition(Some(suffix_id.clone())));
            source.push('\n');
        }

        // Each invocation independently picks a function + generates recursive args
        let invocation_strategy =
            prop::sample::select(function_defs).prop_flat_map(move |(suffix_id, equality)| {
                let fn_id = format!("{}-{}", equality.descriptor(), suffix_id);
                let arity = equality.arity();
                let reg = registry.clone();

                prop::collection::vec(recursive_arg_strategy(registry_vec.clone()), arity..=arity)
                    .prop_map(move |args| {
                        let args_str = args
                            .iter()
                            .map(|a| a.to_metta_string())
                            .collect::<Vec<_>>()
                            .join(" ");

                        let result = equality.eval(args.iter().map(|a| a.eval(&reg)).collect());
                        (format!("! ({} {})\n", fn_id, args_str), result)
                    })
            });

        prop::collection::vec(invocation_strategy, 1..20).prop_map(move |invocations| {
            let mut src = source.clone();
            let mut reductions = Vec::new();
            for (line, result) in invocations {
                src.push_str(&line);
                reductions.push(result);
            }

            NestedTestProgram {
                source: src,
                reductions,
            }
        })
    })
}

/// Generates programs with undefined function calls
fn undefined_function_programs() -> impl Strategy<Value = String> {
    let id_suffix = r"[a-z0-9]{6}";
    let suffix_strategy = string_regex(id_suffix).unwrap();

    (
        suffix_strategy,
        prop::collection::vec(-1000i64..=1000i64, 1..=5),
    )
        .prop_map(|(suffix, args)| {
            let arguments = args
                .iter()
                .map(|x| x.to_string())
                .collect::<Vec<_>>()
                .join(" ");

            format!("! ({} {})\n", suffix, arguments)
        })
}

proptest! {
  #[test]
  fn nested_all_invocations_reduced(test_program in nested_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    for value in &result {
        prop_assert!(
            matches!(value, MettaValue::Long(_)),
            "Expected all outputs to be reduced to Long, but found: {:?}",
            value
        );
    }
  }

  #[test]
  fn nested_reductions_count_matches_invocations(test_program in nested_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    prop_assert_eq!(result.len(), test_program.reductions.len());
  }

  #[test]
  fn nested_reductions_match_oracle(test_program in nested_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    let actual: Vec<i64> = result
        .iter()
        .map(|v| match v {
            MettaValue::Long(n) => *n,
            other => panic!("Expected Long, got {:?}", other),
        })
        .collect();

    prop_assert_eq!(test_program.reductions, actual);
  }

  #[test]
  fn reductions_total_value_are_equal_to_expected(test_program in unary_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    let expected_total_val: i64 = test_program.invocations.iter().sum();

    let mut actual_total_val = 0;
    for metta_val in result {
      match metta_val {
        MettaValue::Long(number) => { actual_total_val += number },
        other => prop_assert!(false, "Expected Long, got {:?}", other),
      }
    }

    prop_assert_eq!(expected_total_val, actual_total_val);
  }

  #[test]
  fn reductions_number_are_equal_to_expected(test_program in unary_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    prop_assert_eq!(result.len(), test_program.invocations.len());
  }

  #[test]
  fn all_invocations_reduced(test_program in unary_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    for value in &result {
        prop_assert!(
            // Choosing Long as acceptable instance for testing polymorphic properties
            matches!(value, mettatron::MettaValue::Long(_)),
            "Expected all outputs to be reduced to Long, but found: {:?}",
            value
        );
    }
  }

  #[test]
  fn multiple_functions_reductions_total_value_are_equal_to_expected(test_program in multiple_flat_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    let expected_total_val: i64 = test_program.invocations.iter().sum();

    for value in &result {
        prop_assert!(
            matches!(value, mettatron::MettaValue::Long(_)),
            "Expected all outputs to be Long, got: {:?}",
            value
        );
    }
    let actual_total_val: i64 = result
        .iter()
        .map(|v| match v {
            mettatron::MettaValue::Long(n) => *n,
            _ => unreachable!("already asserted all are Long"),
        })
        .sum();

    prop_assert_eq!(expected_total_val, actual_total_val);
  }

  #[test]
  fn multiple_functions_reductions_number_are_equal_to_expected(test_program in multiple_flat_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    prop_assert_eq!(result.len(), test_program.invocations.len());
  }

  #[test]
  fn multiple_functions_all_invocations_reduced(test_program in multiple_flat_invocations()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

    for value in &result {
        prop_assert!(
            matches!(value, mettatron::MettaValue::Long(_)),
            "Expected all outputs to be reduced to Long, but found: {:?}",
            value
        );
    }
  }

  #[test]
  fn undefined_function_not_reduced(src in undefined_function_programs()) {
    let state = MettaState::new_empty();
    let compiled = compile(&src);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
        .expect("Failed to evaluate")
        .output;

        for value in &result {
          prop_assert!(
              matches!(value, mettatron::MettaValue::SExpr(_)),
              "Expected all outputs to be not reduced {:?}",
              value
          );
      }
  }
}
