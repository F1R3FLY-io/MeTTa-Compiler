//! GC Root Audit Tool for MeTTaTron
//!
//! Walks the Rust AST of MeTTaTron to systematically find ALL `MettaValue`
//! storage locations, ALL `impl RootProvider` blocks, and ALL
//! `register_root_provider()` calls. Cross-references to find unregistered roots.
//!
//! ALL classification is dynamic — derived from AST scanning only.
//! No hardcoded safe lists.
//!
//! # Usage
//!
//! ```bash
//! # Terminal output (default)
//! cargo run -- ../../src/
//!
//! # JSON output (for CI)
//! cargo run -- ../../src/ --format json
//!
//! # Markdown output (for docs)
//! cargo run -- ../../src/ --format markdown
//!
//! # Include test code
//! cargo run -- ../../src/ --include-tests
//! ```

mod classifier;
mod report;
mod scanner;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "gc-root-audit")]
#[command(about = "Find unregistered GC roots in MeTTaTron — fully dynamic AST analysis")]
struct Args {
    /// Source directory to scan
    #[arg(default_value = "../../src")]
    source_dir: PathBuf,

    /// Output format
    #[arg(long, default_value = "terminal")]
    format: report::OutputFormat,

    /// Include test code (#[cfg(test)] blocks)
    #[arg(long, default_value_t = false)]
    include_tests: bool,

    /// Show all categories, not just issues
    #[arg(long, default_value_t = false)]
    verbose: bool,
}

fn main() {
    let args = Args::parse();

    // Pass 1: Scan all .rs files — find ALL MettaValue storage and ALL RootProvider impls
    eprintln!("Pass 1: Scanning {} ...", args.source_dir.display());
    let scan_result = scanner::scan_directory(&args.source_dir, args.include_tests);
    eprintln!(
        "  Found {} files, {} type definitions, {} impl blocks, {} statics, {} type aliases",
        scan_result.files_scanned,
        scan_result.type_defs.len(),
        scan_result.impl_blocks.len(),
        scan_result.statics.len(),
        scan_result.type_aliases.len(),
    );
    eprintln!(
        "  RootProvider types (from AST): {:?}",
        scan_result.root_provider_types
    );

    // Pass 2: Classify each occurrence — ALL classification is dynamic
    eprintln!("Pass 2: Classifying MettaValue storage locations (fully dynamic) ...");
    let classified = classifier::classify(&scan_result);
    let category_counts = classifier::count_by_category(&classified);
    for (cat, count) in &category_counts {
        eprintln!("  {:?}: {}", cat, count);
    }

    // Pass 3: Report
    eprintln!("Pass 3: Generating report ...");
    report::output(&classified, args.format, args.verbose);

    // Exit with error code if there are unregistered roots or unguarded frame chain Vecs
    let unregistered_count = classified
        .iter()
        .filter(|l| matches!(l.category, classifier::Category::PersistentUnregistered))
        .count();
    let frame_chain_count = classified
        .iter()
        .filter(|l| matches!(l.category, classifier::Category::FrameChainMissing))
        .count();
    let unknown_count = classified
        .iter()
        .filter(|l| matches!(l.category, classifier::Category::Unknown))
        .count();

    if unregistered_count > 0 {
        eprintln!(
            "\nERROR: Found {} PERSISTENT_UNREGISTERED GC root(s)!",
            unregistered_count
        );
        std::process::exit(1);
    }
    if frame_chain_count > 0 {
        eprintln!(
            "\nERROR: Found {} FRAME_CHAIN_MISSING — Vec(s) live across eval_trampoline_generic without maybe_push_frame!",
            frame_chain_count
        );
        std::process::exit(1);
    }
    if unknown_count > 0 {
        eprintln!(
            "\nWARNING: Found {} UNKNOWN GC root location(s) that need manual review.",
            unknown_count
        );
        // Don't fail on UNKNOWN — these are just informational
    }

    if unregistered_count == 0 && frame_chain_count == 0 {
        eprintln!("\nAll detected persistent MettaValue storage locations have GC root coverage.");
        eprintln!("All Vec locals across eval_trampoline_generic calls are frame-chain guarded.");
    }
}
