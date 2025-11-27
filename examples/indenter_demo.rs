//! Demonstrates the SmartIndenter API
//!
//! Shows how to calculate indentation levels for multi-line MeTTa expressions
//!
//! Run with: cargo run --example indenter_demo

use mettatron::repl::SmartIndenter;

fn main() {
    let mut indenter = SmartIndenter::new().expect("Failed to create indenter");

    println!("SmartIndenter Demo");
    println!("==================\n");

    // Example 1: Simple incomplete expression
    let input1 = "(+ 1";
    let indent1 = indenter.calculate_indent(input1);
    println!("Input:  \"{}\"", input1);
    println!("Indent: {} spaces (1 unclosed paren)\n", indent1);

    // Example 2: Nested incomplete expression
    let input2 = "(foo (bar";
    let indent2 = indenter.calculate_indent(input2);
    println!("Input:  \"{}\"", input2);
    println!("Indent: {} spaces (2 unclosed parens)\n", indent2);

    // Example 3: Complete expression (no indent needed)
    let input3 = "(+ 1 2)";
    let indent3 = indenter.calculate_indent(input3);
    println!("Input:  \"{}\"", input3);
    println!("Indent: {} spaces (all closed)\n", indent3);

    // Example 4: String with delimiter inside (ignored)
    let input4 = r#"(print "("#;
    let indent4 = indenter.calculate_indent(input4);
    println!("Input:  \"{}\"", input4);
    println!("Indent: {} spaces (delimiter in string ignored)\n", indent4);

    // Example 5: Comment with delimiter (ignored)
    let input5 = "(foo ; comment with (\nbar";
    let indent5 = indenter.calculate_indent(input5);
    println!("Input:  \"{}\"", input5.replace('\n', "\\n"));
    println!(
        "Indent: {} spaces (delimiter in comment ignored)\n",
        indent5
    );

    // Example 6: Mixed delimiters
    let input6 = "(foo {bar";
    let indent6 = indenter.calculate_indent(input6);
    println!("Input:  \"{}\"", input6);
    println!("Indent: {} spaces (1 paren + 1 brace)\n", indent6);

    // Example 7: Demonstrate continuation prompt
    println!("\nContinuation Prompt Demo:");
    println!("=========================");
    let buffer = "(define (fibonacci n)";
    let prompt = indenter.continuation_prompt(buffer, "...> ");
    println!("Buffer: \"{}\"", buffer);
    println!(
        "Prompt: \"{}\" (with {} spaces)",
        prompt,
        indenter.calculate_indent(buffer)
    );
    println!(
        "        {}<- continuation starts here",
        " ".repeat(prompt.len())
    );

    // Example 8: Custom indent width
    println!("\nCustom Indent Width:");
    println!("====================");
    let mut indenter_4space = SmartIndenter::with_indent_width(4).unwrap();
    let input8 = "(foo (bar";
    let indent8 = indenter_4space.calculate_indent(input8);
    println!("Input:  \"{}\"", input8);
    println!(
        "Indent: {} spaces (4-space mode, 2 unclosed parens)",
        indent8
    );

    println!("\nNote: The REPL has SmartIndenter integrated, accessible via:");
    println!("  helper.calculate_indent(buffer)");
    println!("  helper.indenter_mut().set_indent_width(4)");
}
