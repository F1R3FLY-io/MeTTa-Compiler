//! Pass 3: Report generation
//!
//! Outputs classified MettaValue locations in terminal (colored), JSON, or markdown format.

use std::str::FromStr;

use colored::Colorize;
use serde::Serialize;

use crate::classifier::{Category, ClassifiedLocation, LocationContext};

/// Output format for the report
#[derive(Debug, Clone)]
pub enum OutputFormat {
    Terminal,
    Json,
    Markdown,
}

impl FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "terminal" => Ok(OutputFormat::Terminal),
            "json" => Ok(OutputFormat::Json),
            "markdown" | "md" => Ok(OutputFormat::Markdown),
            _ => Err(format!(
                "Unknown format: {}. Use terminal, json, or markdown",
                s
            )),
        }
    }
}

/// JSON-serializable report entry
#[derive(Debug, Serialize)]
struct JsonEntry {
    file: String,
    line: usize,
    category: String,
    context_type: String,
    name: String,
    ty: String,
    reason: String,
}

/// Generate output in the specified format
pub fn output(locations: &[ClassifiedLocation], format: OutputFormat, verbose: bool) {
    match format {
        OutputFormat::Terminal => output_terminal(locations, verbose),
        OutputFormat::Json => output_json(locations, verbose),
        OutputFormat::Markdown => output_markdown(locations, verbose),
    }
}

fn category_str(cat: &Category) -> &'static str {
    match cat {
        Category::RegisteredRoot => "REGISTERED_ROOT",
        Category::GenericEvalType => "GENERIC_EVAL_TYPE",
        Category::PersistentUnregistered => "PERSISTENT_UNREGISTERED",
        Category::GcInfrastructure => "GC_INFRASTRUCTURE",
        Category::FrameChainMissing => "FRAME_CHAIN_MISSING",
        Category::FieldNotCollected => "FIELD_NOT_COLLECTED",
        Category::FieldPartiallyCollected => "FIELD_PARTIALLY_COLLECTED",
        Category::Unknown => "UNKNOWN",
    }
}

fn location_name(ctx: &LocationContext) -> String {
    match ctx {
        LocationContext::StaticVar { name, .. } => format!("static {}", name),
        LocationContext::StructField {
            struct_name,
            field_name,
            ..
        } => format!("{}.{}", struct_name, field_name),
        LocationContext::ThreadLocal { name, .. } => format!("thread_local! {}", name),
        LocationContext::TypeAlias { name, .. } => format!("type {}", name),
    }
}

fn location_ty(ctx: &LocationContext) -> &str {
    match ctx {
        LocationContext::StaticVar { ty, .. } => ty,
        LocationContext::StructField { ty, .. } => ty,
        LocationContext::ThreadLocal { ty, .. } => ty,
        LocationContext::TypeAlias { target, .. } => target,
    }
}

fn output_terminal(locations: &[ClassifiedLocation], verbose: bool) {
    let mut by_category: std::collections::BTreeMap<&Category, Vec<&ClassifiedLocation>> =
        std::collections::BTreeMap::new();
    for loc in locations {
        by_category.entry(&loc.category).or_default().push(loc);
    }

    // Always show error/warning categories
    let priority_categories = [
        Category::PersistentUnregistered,
        Category::FrameChainMissing,
        Category::FieldNotCollected,
        Category::FieldPartiallyCollected,
        Category::Unknown,
        Category::GcInfrastructure,
    ];
    for cat in &priority_categories {
        if let Some(locs) = by_category.get(cat) {
            let header = format!("\n=== {} ({}) ===", category_str(cat), locs.len());
            match cat {
                Category::PersistentUnregistered
                | Category::FrameChainMissing
                | Category::FieldNotCollected => println!("{}", header.red().bold()),
                Category::FieldPartiallyCollected => println!("{}", header.yellow().bold()),
                Category::Unknown => println!("{}", header.yellow().bold()),
                Category::GcInfrastructure => println!("{}", header.cyan()),
                _ => println!("{}", header),
            }

            for loc in locs {
                let name = location_name(&loc.context);
                let ty = location_ty(&loc.context);
                match cat {
                    Category::PersistentUnregistered
                    | Category::FrameChainMissing
                    | Category::FieldNotCollected => {
                        println!(
                            "  {} {}:{} — {} [{}]",
                            "BUG".red().bold(),
                            loc.file,
                            loc.line,
                            name.red(),
                            ty
                        );
                        println!("      {}", loc.reason.red());
                    }
                    Category::FieldPartiallyCollected => {
                        println!(
                            "  {} {}:{} — {} [{}]",
                            "WARN".yellow().bold(),
                            loc.file,
                            loc.line,
                            name.yellow(),
                            ty
                        );
                        println!("      {}", loc.reason.yellow());
                    }
                    Category::Unknown => {
                        println!(
                            "  {} {}:{} — {} [{}]",
                            "???".yellow(),
                            loc.file,
                            loc.line,
                            name.yellow(),
                            ty
                        );
                        println!("      {}", loc.reason);
                    }
                    Category::GcInfrastructure => {
                        println!(
                            "  {} {}:{} — {} [{}]",
                            "GC".cyan(),
                            loc.file,
                            loc.line,
                            name.cyan(),
                            ty
                        );
                        println!("      {}", loc.reason.cyan());
                    }
                    _ => {
                        println!("  {}:{} — {} [{}]", loc.file, loc.line, name, ty);
                    }
                }
            }
        }
    }

    if verbose {
        for (cat, locs) in &by_category {
            if priority_categories.contains(cat) {
                continue;
            }
            println!("\n=== {} ({}) ===", category_str(cat).green(), locs.len());
            for loc in locs {
                let name = location_name(&loc.context);
                let ty = location_ty(&loc.context);
                println!(
                    "  {} {}:{} — {} [{}]",
                    "OK".green(),
                    loc.file,
                    loc.line,
                    name,
                    ty
                );
            }
        }
    }
}

fn output_json(locations: &[ClassifiedLocation], verbose: bool) {
    let entries: Vec<JsonEntry> = locations
        .iter()
        .filter(|l| {
            verbose
                || matches!(
                    l.category,
                    Category::PersistentUnregistered
                        | Category::FrameChainMissing
                        | Category::Unknown
                        | Category::GcInfrastructure
                )
        })
        .map(|l| JsonEntry {
            file: l.file.clone(),
            line: l.line,
            category: category_str(&l.category).to_string(),
            context_type: match &l.context {
                LocationContext::StaticVar { .. } => "static",
                LocationContext::StructField { .. } => "struct_field",
                LocationContext::ThreadLocal { .. } => "thread_local",
                LocationContext::TypeAlias { .. } => "type_alias",
            }
            .to_string(),
            name: location_name(&l.context),
            ty: location_ty(&l.context).to_string(),
            reason: l.reason.clone(),
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string_pretty(&entries).expect("JSON serialization should not fail")
    );
}

fn output_markdown(locations: &[ClassifiedLocation], verbose: bool) {
    println!("# GC Root Audit Report\n");

    let mut counts = std::collections::BTreeMap::new();
    for loc in locations {
        *counts.entry(loc.category.clone()).or_insert(0usize) += 1;
    }
    println!("## Summary\n");
    println!("| Category | Count |");
    println!("|---|---|");
    for (cat, count) in &counts {
        println!("| {} | {} |", category_str(cat), count);
    }
    println!();

    let issues: Vec<_> = locations
        .iter()
        .filter(|l| {
            matches!(
                l.category,
                Category::PersistentUnregistered | Category::Unknown | Category::GcInfrastructure
            )
        })
        .collect();

    if !issues.is_empty() {
        println!("## Issues\n");
        println!("| File | Line | Location | Type | Category | Reason |");
        println!("|---|---|---|---|---|---|");
        for loc in &issues {
            let name = location_name(&loc.context);
            let ty = location_ty(&loc.context);
            println!(
                "| `{}` | {} | `{}` | `{}` | **{}** | {} |",
                loc.file,
                loc.line,
                name,
                ty,
                category_str(&loc.category),
                loc.reason
            );
        }
        println!();
    }

    if verbose {
        let safe: Vec<_> = locations
            .iter()
            .filter(|l| {
                !matches!(
                    l.category,
                    Category::PersistentUnregistered
                        | Category::FrameChainMissing
                        | Category::Unknown
                        | Category::GcInfrastructure
                )
            })
            .collect();

        if !safe.is_empty() {
            println!("## Safe Locations\n");
            println!("| File | Line | Location | Type | Category |");
            println!("|---|---|---|---|---|");
            for loc in &safe {
                let name = location_name(&loc.context);
                let ty = location_ty(&loc.context);
                println!(
                    "| `{}` | {} | `{}` | `{}` | {} |",
                    loc.file,
                    loc.line,
                    name,
                    ty,
                    category_str(&loc.category)
                );
            }
        }
    }
}
