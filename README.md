# MeTTa-Compiler
Compiler from MeTTa to MeTTa IL

# Prerequisites for parser
- BNF Converter (BNFC in short) ( https://bnfc.digitalgrammars.com )
- C2Rust ( https://github.com/immunant/c2rust )
- Flex: ( "sudo apt-get install flex" on Linux; "brew install flex" on macOS )
- Bison: ( "sudo apt-get install bison" on Linux; "brew install bison" on macOS )
- gcc: ( "sudo apt install build-essential" on Linux; "brew install gcc" on macOS )
- BNFC grammar file "parser/bnfc/rust/Grammar.cf" for building parser
  ( such as, https://gist.github.com/leithaus/954de3e97593525ace3dd2a7999e28f1 )

# Build and run
./build_rust_parser
