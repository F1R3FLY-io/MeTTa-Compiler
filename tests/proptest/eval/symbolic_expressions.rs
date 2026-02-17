use mettatron::{compile, run_state, MettaState};
use proptest::prelude::*;
use proptest::string::string_regex;

#[derive(Clone, Debug)]
struct SymbolicReductionTestProgram {
    source: String,

    // oracle
    reduced_exprs: Vec<String>,
}

fn basic_symbolic_expression() -> impl Strategy<Value = String> {
    let word = string_regex(r"[A-Za-z0-9_-]{1,30}").unwrap();
    prop::collection::vec(word, 1..=10).prop_map(|words| words.join(" "))
}

/// Generates a sequance of basic symbolic expressions like
/// ! (abcd)
/// ! (kj2-n ams)
/// Oracle collects a set of expected unreduced expressions
fn basic_symbolic_expression_set() -> impl Strategy<Value = SymbolicReductionTestProgram> {
    prop::collection::vec(basic_symbolic_expression(), 1..5).prop_map(|expressions| {
        let mut oracle: Vec<String> = vec![];
        let mut res = String::new();

        for expr in expressions {
            res.push_str(&format!("! ({}) \n ", expr));
            oracle.push(format!("({})", expr));
        }

        return SymbolicReductionTestProgram {
            source: res,
            reduced_exprs: oracle,
        };
    })
}

#[derive(Clone, Debug)]
struct EqualityReductionTestProgram {
    source: String,

    // oracle
    equality_id: String,
    equality_matches_ctr: usize,
}

fn static_argument() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("A".to_string()),
        Just("B".to_string()),
        Just("C".to_string()),
        Just("D".to_string()),
    ]
}

fn equality() -> impl Strategy<Value = (String, String, String)> {
    static_argument().prop_map(|arg| {
        let eq_descriptor = format!("only {}", arg);
        let eq_source = format!("(= ({}) (Input {} is accepted))", eq_descriptor, arg);
        (arg, eq_descriptor, eq_source)
    })
}

/// Generates reduction attempt based on one of the `static_argument`
fn reduction() -> impl Strategy<Value = (String, String)> {
    static_argument().prop_map(|arg| {
        let rd_descriptor = format!("only {}", arg);
        let rd_source = format!("! ({})", rd_descriptor);

        (rd_descriptor, rd_source)
    })
}

/// Generates equality and a bunch of equality reduction attempts:
/// (= (only-a A) (Input A is accepted))
/// ! (only-a A)
/// ! (only-a B)
/// Oracle collects as a counter of given equality occurrences
fn equality_reduction_appeared_with_counter() -> impl Strategy<Value = EqualityReductionTestProgram>
{
    (equality(), prop::collection::vec(reduction(), 1..50)).prop_map(
        |((arg, eq_descriptor, eq_source), reductions)| {
            let mut match_counter_oracle: usize = 0;

            let mut res = String::new();
            res.push_str(&eq_source);
            res.push('\n');

            for (rd_descriptor, rd_source) in reductions {
                if rd_descriptor == eq_descriptor {
                    match_counter_oracle += 1;
                }
                res.push_str(&rd_source);
                res.push('\n');
            }

            EqualityReductionTestProgram {
                source: res,
                equality_id: arg,
                equality_matches_ctr: match_counter_oracle,
            }
        },
    )
}

proptest! {
  #[test]
  fn basic_symbolic_reduction(test_program in basic_symbolic_expression_set()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
      .expect("Failed to evaluate")
      .output;

    for i in 0..result.len() {
      let res_val = result[i].clone().to_metta_string();
      let expected_res = test_program.reduced_exprs[i].clone();

      prop_assert_eq!(res_val, expected_res);
    }
  }


  #[test]
  fn equality_reduction_appeared_N_times(test_program in equality_reduction_appeared_with_counter()) {
    let state = MettaState::new_empty();
    let compiled = compile(&test_program.source);
    prop_assert!(compiled.is_ok());

    let result = run_state(state, compiled.unwrap())
      .expect("Failed to evaluate")
      .output;

    // Basically just count how many times expression X gets mached reductions attempts of the same expression X
    let mut actual_counter = 0;
    for output in result {
        let expected_output = format!("(Input {} is accepted)", test_program.equality_id);
        if expected_output == output.to_metta_string() {
            actual_counter += 1;
        }
    }

    prop_assert_eq!(actual_counter, test_program.equality_matches_ctr);
  }
}
