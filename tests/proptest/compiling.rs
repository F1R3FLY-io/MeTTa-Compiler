use mettatron::compile;
use proptest::prelude::*;

use super::strategies::conjunction::*;
use super::strategies::primitive::*;
use super::strategies::sexpr::*;

proptest! {
  #[test]
  fn compiled_arb_empty_string(src in empty_strings(20)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn compiled_atom(src in primitive()) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn compiled_valid_flat_sexpr(src in valid_flat_sexpr(100)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn not_compiled_invalid_flat_sexpr(src in invalid_flat_sexpr(100)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_err());
  }

  #[test]
  fn compiled_valid_nested_sexpr(src in valid_nested_sexpr(8, 256, 10)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn compiled_valid_deeply_nested_sexpr(src in valid_nested_sexpr(24, 512, 30)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn not_compiled_invalid_nested_sexpr(src in invalid_nested_sexpr(8, 256, 10)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_err());
  }

  #[test]
  fn compiled_multiline_sexpr(src in multiline_sexpr()) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn compiled_valid_flat_conjunction(src in valid_flat_conjunction(100)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn not_compiled_invalid_flat_conjunction(src in invalid_flat_conjunction(100)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_err());
  }

  #[test]
  fn compiled_valid_nested_conjunction(src in valid_nested_conjunction(8, 256, 10)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn not_compiled_invalid_nested_conjunction(src in invalid_nested_conjunction(8, 256, 10)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_err());
  }

  #[test]
  fn compiled_mixed_conjunction_sexpr(src in mixed_conjunction_sexpr(8, 256, 10)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn compiled_empty_conjunction(src in empty_conjunction()) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn compiled_unary_conjunction(src in unary_conjunction()) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn compiled_multiline_conjunction(src in multiline_conjunction()) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }
}
