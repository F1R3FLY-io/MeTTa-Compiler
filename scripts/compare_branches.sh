#!/bin/bash
# Branch Comparison Benchmark Script
# Compares performance between current branch and main branch
#
# Usage:
#   ./scripts/compare_branches.sh [main_branch_name]
#
# Default main_branch_name is "main"

set -e  # Exit on error

# Configuration
CURRENT_BRANCH=$(git branch --show-current)
MAIN_BRANCH="${1:-main}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESULTS_DIR="benchmark_results_${TIMESTAMP}"
CPU_AFFINITY="taskset -c 0-17"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo "========================================="
echo "Branch Comparison Benchmark"
echo "========================================="
echo "Current Branch: $CURRENT_BRANCH"
echo "Main Branch: $MAIN_BRANCH"
echo "Results Directory: $RESULTS_DIR"
echo ""

# Create results directory
mkdir -p "$RESULTS_DIR"

# Function to run benchmarks with proper settings
run_benchmarks() {
    local branch_name=$1
    local output_file=$2

    echo -e "${YELLOW}Running benchmarks on branch: $branch_name${NC}"

    # Run comprehensive branch_comparison benchmark
    echo "  - Running branch_comparison benchmark..."
    $CPU_AFFINITY cargo bench --bench branch_comparison 2>&1 | tee "$output_file"

    # Capture build information
    echo -e "\n\n=== BUILD INFORMATION ===" >> "$output_file"
    echo "Git Branch: $branch_name" >> "$output_file"
    echo "Git Commit: $(git rev-parse HEAD)" >> "$output_file"
    echo "Git Commit Message: $(git log -1 --pretty=%B)" >> "$output_file"
    cargo --version >> "$output_file"
    rustc --version >> "$output_file"

    # Binary size
    if [ -f "target/release/mettatron" ]; then
        echo -e "\n=== BINARY SIZE ===" >> "$output_file"
        ls -lh target/release/mettatron >> "$output_file"
    fi

    echo -e "${GREEN}  ✓ Benchmarks complete${NC}"
}

# Benchmark current branch
echo -e "${GREEN}Step 1/3: Benchmarking current branch ($CURRENT_BRANCH)${NC}"
run_benchmarks "$CURRENT_BRANCH" "$RESULTS_DIR/current_branch.txt"

# Stash any uncommitted changes
HAS_CHANGES=false
if ! git diff-index --quiet HEAD --; then
    echo -e "${YELLOW}Stashing uncommitted changes...${NC}"
    git stash push -m "Temporary stash for benchmark comparison"
    HAS_CHANGES=true
fi

# Checkout main branch
echo -e "${GREEN}Step 2/3: Switching to main branch ($MAIN_BRANCH)${NC}"
git checkout "$MAIN_BRANCH"

# Clean and rebuild
echo "  - Cleaning build artifacts..."
cargo clean --quiet

# Benchmark main branch
run_benchmarks "$MAIN_BRANCH" "$RESULTS_DIR/main_branch.txt"

# Return to original branch
echo -e "${GREEN}Step 3/3: Returning to original branch ($CURRENT_BRANCH)${NC}"
git checkout "$CURRENT_BRANCH"

# Restore stashed changes if any
if [ "$HAS_CHANGES" = true ]; then
    echo -e "${YELLOW}Restoring stashed changes...${NC}"
    git stash pop
fi

# Clean and rebuild current branch
echo "  - Rebuilding current branch..."
cargo build --release --quiet

# Generate comparison report
echo ""
echo "========================================="
echo "Generating Comparison Report"
echo "========================================="

# Check if Python analysis script exists
if [ -f "scripts/analyze_benchmark_results.py" ]; then
    echo "Running Python analysis..."
    python3 scripts/analyze_benchmark_results.py \
        "$RESULTS_DIR/main_branch.txt" \
        "$RESULTS_DIR/current_branch.txt" \
        > "$RESULTS_DIR/comparison_report.md"
    echo -e "${GREEN}✓ Comparison report generated${NC}"
else
    echo -e "${YELLOW}⚠ Python analysis script not found, generating basic report${NC}"

    # Basic comparison (just show file locations)
    cat > "$RESULTS_DIR/comparison_report.md" <<EOF
# Benchmark Comparison Report

**Date**: $(date)
**Current Branch**: $CURRENT_BRANCH
**Main Branch**: $MAIN_BRANCH

## Results Files

- **Main Branch**: $RESULTS_DIR/main_branch.txt
- **Current Branch**: $RESULTS_DIR/current_branch.txt

## Manual Analysis Required

Run the following command to analyze results:

\`\`\`bash
python3 scripts/analyze_benchmark_results.py \\
    $RESULTS_DIR/main_branch.txt \\
    $RESULTS_DIR/current_branch.txt
\`\`\`

## Quick Comparison

### Main Branch Summary
\`\`\`
$(grep "time:" "$RESULTS_DIR/main_branch.txt" | head -10)
\`\`\`

### Current Branch Summary
\`\`\`
$(grep "time:" "$RESULTS_DIR/current_branch.txt" | head -10)
\`\`\`
EOF
fi

echo ""
echo "========================================="
echo "Comparison Complete!"
echo "========================================="
echo -e "${GREEN}Results saved to: $RESULTS_DIR/${NC}"
echo ""
echo "Files:"
echo "  - Main branch results:    $RESULTS_DIR/main_branch.txt"
echo "  - Current branch results: $RESULTS_DIR/current_branch.txt"
echo "  - Comparison report:      $RESULTS_DIR/comparison_report.md"
echo ""
echo "View the comparison report:"
echo "  cat $RESULTS_DIR/comparison_report.md"
echo ""

# Display quick summary of key benchmarks
echo "Key Benchmark Summary:"
echo "----------------------"
echo "Prefix Fast Path (has_sexpr_fact_ground):"
grep -A 1 "has_sexpr_fact_ground/10000" "$RESULTS_DIR/main_branch.txt" | grep "time:" || echo "  Main: Not found"
grep -A 1 "has_sexpr_fact_ground/10000" "$RESULTS_DIR/current_branch.txt" | grep "time:" || echo "  Current: Not found"

echo ""
echo "Bulk Insertion (add_facts_bulk/1000):"
grep -A 1 "add_facts_bulk/1000" "$RESULTS_DIR/main_branch.txt" | grep "time:" || echo "  Main: Not found"
grep -A 1 "add_facts_bulk/1000" "$RESULTS_DIR/current_branch.txt" | grep "time:" || echo "  Current: Not found"

echo ""
echo -e "${GREEN}Done!${NC}"
