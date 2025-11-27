#!/usr/bin/env python3
"""
Analyze and compare Criterion benchmark results between two branches.

Usage:
    python3 analyze_benchmark_results.py main_results.txt current_results.txt

Outputs a markdown comparison report to stdout.
"""

import sys
import re
from typing import Dict, List, Tuple

def parse_criterion_output(filename: str) -> Dict[str, Dict[str, str]]:
    """Parse Criterion benchmark output and extract timing information."""
    benchmarks = {}

    with open(filename, 'r') as f:
        content = f.read()

    # Pattern to match benchmark names and their times
    # Example: "prefix_fast_path/has_sexpr_fact_ground/10000   time:   [123.45 µs 124.56 µs 125.67 µs]"
    pattern = r'(\S+/\S+/\d+|[\w_]+)\s+time:\s+\[([^\]]+)\]'

    for match in re.finditer(pattern, content):
        bench_name = match.group(1)
        time_info = match.group(2)

        # Extract the median time (middle value)
        times = time_info.split()
        if len(times) >= 2:
            median_time = times[1]  # Second value is typically the median
            benchmarks[bench_name] = {
                'time': median_time,
                'full_time': time_info
            }

    return benchmarks

def parse_time_to_ns(time_str: str) -> float:
    """Convert time string (e.g., '123.45 µs') to nanoseconds."""
    match = re.match(r'([\d.]+)\s*(\S+)', time_str)
    if not match:
        return 0.0

    value = float(match.group(1))
    unit = match.group(2)

    # Convert to nanoseconds
    conversions = {
        'ns': 1.0,
        'µs': 1000.0,
        'us': 1000.0,
        'ms': 1000000.0,
        's': 1000000000.0,
    }

    return value * conversions.get(unit, 1.0)

def calculate_speedup(main_ns: float, current_ns: float) -> Tuple[float, str]:
    """Calculate speedup ratio and return formatted string."""
    if main_ns == 0 or current_ns == 0:
        return 0.0, "N/A"

    ratio = main_ns / current_ns

    if ratio > 1.0:
        # Current is faster
        return ratio, f"{ratio:.2f}× faster"
    elif ratio < 1.0:
        # Current is slower (regression)
        regression = 1.0 / ratio
        return ratio, f"{regression:.2f}× slower (regression)"
    else:
        return 1.0, "Same"

def main():
    if len(sys.argv) != 3:
        print("Usage: python3 analyze_benchmark_results.py main_results.txt current_results.txt",
              file=sys.stderr)
        sys.exit(1)

    main_file = sys.argv[1]
    current_file = sys.argv[2]

    # Parse both result files
    main_results = parse_criterion_output(main_file)
    current_results = parse_criterion_output(current_file)

    # Generate markdown report
    print("# Branch Comparison Benchmark Report\n")
    print(f"**Main Branch Results**: `{main_file}`")
    print(f"**Current Branch Results**: `{current_file}`\n")

    # Get all unique benchmark names
    all_benchmarks = sorted(set(main_results.keys()) | set(current_results.keys()))

    # Categorize benchmarks
    categories = {
        'prefix_fast_path': [],
        'bulk_insertion': [],
        'cow_clone': [],
        'pattern_matching': [],
        'rule_matching': [],
        'type_lookup': [],
        'evaluation': [],
        'scalability': [],
        'other': []
    }

    for bench in all_benchmarks:
        categorized = False
        for category in categories.keys():
            if category in bench:
                categories[category].append(bench)
                categorized = True
                break
        if not categorized:
            categories['other'].append(bench)

    # Print results by category
    for category, benches in categories.items():
        if not benches:
            continue

        print(f"## {category.replace('_', ' ').title()}\n")
        print("| Benchmark | Main Branch | Current Branch | Speedup |")
        print("|-----------|-------------|----------------|---------|")

        for bench in benches:
            main_time = main_results.get(bench, {}).get('time', 'N/A')
            current_time = current_results.get(bench, {}).get('time', 'N/A')

            # Calculate speedup
            if main_time != 'N/A' and current_time != 'N/A':
                main_ns = parse_time_to_ns(main_time)
                current_ns = parse_time_to_ns(current_time)
                ratio, speedup_str = calculate_speedup(main_ns, current_ns)

                # Add emoji indicators
                if ratio > 1.1:
                    speedup_str = f"✅ {speedup_str}"
                elif ratio < 0.9:
                    speedup_str = f"❌ {speedup_str}"
                else:
                    speedup_str = f"➖ {speedup_str}"
            else:
                speedup_str = "N/A"

            print(f"| `{bench}` | {main_time} | {current_time} | {speedup_str} |")

        print()

    # Summary statistics
    print("## Summary\n")

    improvements = 0
    regressions = 0
    similar = 0

    for bench in all_benchmarks:
        main_time = main_results.get(bench, {}).get('time', 'N/A')
        current_time = current_results.get(bench, {}).get('time', 'N/A')

        if main_time != 'N/A' and current_time != 'N/A':
            main_ns = parse_time_to_ns(main_time)
            current_ns = parse_time_to_ns(current_time)
            ratio, _ = calculate_speedup(main_ns, current_ns)

            if ratio > 1.1:
                improvements += 1
            elif ratio < 0.9:
                regressions += 1
            else:
                similar += 1

    total = improvements + regressions + similar
    print(f"- **Total benchmarks**: {total}")
    print(f"- **Improvements** (>10% faster): {improvements} ✅")
    print(f"- **Regressions** (>10% slower): {regressions} ❌")
    print(f"- **Similar** (±10%): {similar} ➖")

    if regressions > 0:
        print("\n⚠️ **WARNING**: Performance regressions detected!")
    elif improvements > 0:
        print("\n🎉 **Success**: Performance improvements detected!")

if __name__ == '__main__':
    main()
