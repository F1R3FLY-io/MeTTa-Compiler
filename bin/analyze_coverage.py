#!/usr/bin/env python3
"""Analyze LCOV coverage report to find files with most uncovered branches."""

import sys
from pathlib import Path
from collections import defaultdict

def parse_lcov(lcov_path):
    """Parse LCOV file and extract branch coverage per file."""
    files = {}
    current_file = None
    branch_total = 0
    branch_hit = 0

    with open(lcov_path) as f:
        for line in f:
            line = line.strip()
            if line.startswith('SF:'):
                current_file = line[3:]
                branch_total = 0
                branch_hit = 0
            elif line.startswith('BRF:'):
                branch_total = int(line[4:])
            elif line.startswith('BRH:'):
                branch_hit = int(line[4:])
            elif line == 'end_of_record':
                if current_file and branch_total > 0:
                    uncovered = branch_total - branch_hit
                    pct = (branch_hit / branch_total) * 100 if branch_total > 0 else 0
                    files[current_file] = {
                        'total': branch_total,
                        'hit': branch_hit,
                        'uncovered': uncovered,
                        'pct': pct
                    }
                current_file = None

    return files

def main():
    lcov_path = sys.argv[1] if len(sys.argv) > 1 else 'target/coverage/lcov.info'

    files = parse_lcov(lcov_path)

    # Calculate totals
    total_branches = sum(f['total'] for f in files.values())
    total_hit = sum(f['hit'] for f in files.values())
    overall_pct = (total_hit / total_branches) * 100 if total_branches > 0 else 0

    print(f"=== Overall Branch Coverage ===")
    print(f"Covered: {total_hit} / {total_branches} ({overall_pct:.2f}%)")
    print(f"Uncovered: {total_branches - total_hit}")
    print()

    # Sort by uncovered branches (most uncovered first)
    sorted_files = sorted(files.items(), key=lambda x: x[1]['uncovered'], reverse=True)

    print("=== Top 30 Files by Uncovered Branches ===")
    print(f"{'Uncovered':<10} {'Coverage':<10} {'File'}")
    print("-" * 80)

    for filepath, data in sorted_files[:30]:
        # Simplify path for display
        if '/MeTTa-Compiler/src/' in filepath:
            short_path = filepath.split('/MeTTa-Compiler/src/')[-1]
        else:
            short_path = filepath
        print(f"{data['uncovered']:<10} {data['pct']:>6.1f}%    {short_path}")

    print()
    print("=== Files with 0% Coverage (>10 branches) ===")
    zero_cov = [(f, d) for f, d in sorted_files if d['pct'] == 0 and d['total'] > 10]
    for filepath, data in zero_cov[:20]:
        if '/MeTTa-Compiler/src/' in filepath:
            short_path = filepath.split('/MeTTa-Compiler/src/')[-1]
        else:
            short_path = filepath
        print(f"{data['uncovered']:<10} branches   {short_path}")

    print()
    print("=== Coverage by Directory ===")
    dir_stats = defaultdict(lambda: {'total': 0, 'hit': 0})
    for filepath, data in files.items():
        if '/MeTTa-Compiler/src/' in filepath:
            short_path = filepath.split('/MeTTa-Compiler/src/')[-1]
            parts = short_path.split('/')
            if len(parts) >= 2:
                dir_key = '/'.join(parts[:2])
            else:
                dir_key = parts[0]
            dir_stats[dir_key]['total'] += data['total']
            dir_stats[dir_key]['hit'] += data['hit']

    sorted_dirs = sorted(dir_stats.items(),
                         key=lambda x: x[1]['total'] - x[1]['hit'],
                         reverse=True)

    print(f"{'Uncovered':<10} {'Coverage':<10} {'Directory'}")
    print("-" * 60)
    for dir_name, data in sorted_dirs[:15]:
        uncovered = data['total'] - data['hit']
        pct = (data['hit'] / data['total']) * 100 if data['total'] > 0 else 0
        print(f"{uncovered:<10} {pct:>6.1f}%    {dir_name}")

if __name__ == '__main__':
    main()
