use mettatron::compile;
use proptest::prelude::*;

use super::generators::*;

proptest! {
  #[test]
  fn compiled_arb_empty_string(src in empty_strings(20)) {
      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }

  #[test]
  fn compiled_atom(src in atom()) {
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
    dbg!(&src);

      let compiled = compile(&src);
      prop_assert!(compiled.is_ok());
  }
}
