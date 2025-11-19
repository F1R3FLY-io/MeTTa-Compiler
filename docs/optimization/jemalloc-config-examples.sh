#!/bin/bash
#
# jemalloc Configuration Examples for MeTTaTron
#
# Purpose: Provides various MALLOC_CONF configurations for different use cases
# Usage: Source this file and call the appropriate function before running your binary
#
# Reference: PATHMAP_JEMALLOC_ANALYSIS.md Section 5.6

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration 1: Conservative (minimal changes)
config_conservative() {
    export MALLOC_CONF="narenas:72"
    echo -e "${GREEN}✓ Applied conservative jemalloc config${NC}"
    echo "  MALLOC_CONF=$MALLOC_CONF"
}

# Configuration 2: Balanced (recommended for production)
config_balanced() {
    export MALLOC_CONF="narenas:72,tcache:true,lg_tcache_max:15,dirty_decay_ms:5000,muzzy_decay_ms:10000"
    echo -e "${GREEN}✓ Applied balanced jemalloc config${NC}"
    echo "  MALLOC_CONF=$MALLOC_CONF"
}

# Configuration 3: Aggressive (maximum concurrency)
config_aggressive() {
    export MALLOC_CONF="narenas:144,tcache:true,lg_tcache_max:16,dirty_decay_ms:10000,muzzy_decay_ms:20000,background_thread:true,max_background_threads:8,metadata_thp:auto"
    echo -e "${GREEN}✓ Applied aggressive jemalloc config${NC}"
    echo "  MALLOC_CONF=$MALLOC_CONF"
}

# Configuration 4: Debugging (enable profiling and stats)
config_debug() {
    export MALLOC_CONF="narenas:72,prof:true,prof_leak:true,lg_prof_sample:20,stats_print:true"
    echo -e "${YELLOW}⚠ Applied debug jemalloc config (performance impact)${NC}"
    echo "  MALLOC_CONF=$MALLOC_CONF"
    echo "  Heap dumps will be written to: jeprof.<pid>.<seq>.heap"
}

# Configuration 5: Low memory overhead
config_low_memory() {
    export MALLOC_CONF="narenas:2,tcache:false,dirty_decay_ms:1000,muzzy_decay_ms:2000"
    echo -e "${GREEN}✓ Applied low-memory jemalloc config${NC}"
    echo "  MALLOC_CONF=$MALLOC_CONF"
    echo "  Warning: May increase lock contention"
}

# Configuration 6: Custom (specify narenas and tcache settings)
config_custom() {
    local narenas=${1:-72}
    local tcache=${2:-true}
    local lg_tcache_max=${3:-15}

    export MALLOC_CONF="narenas:${narenas},tcache:${tcache},lg_tcache_max:${lg_tcache_max}"
    echo -e "${GREEN}✓ Applied custom jemalloc config${NC}"
    echo "  MALLOC_CONF=$MALLOC_CONF"
}

# Verify jemalloc is active
verify_jemalloc() {
    if ! command -v jemalloc-config &> /dev/null; then
        echo -e "${RED}✗ jemalloc not found on system${NC}"
        echo "  Install with: sudo pacman -S jemalloc (Arch Linux)"
        return 1
    fi

    local version=$(jemalloc-config --version)
    echo -e "${GREEN}✓ jemalloc version: ${version}${NC}"

    # Check if binary uses jemalloc
    if [ -n "${1:-}" ] && [ -f "$1" ]; then
        if ldd "$1" | grep -q jemalloc; then
            echo -e "${GREEN}✓ Binary $1 uses jemalloc${NC}"
        else
            echo -e "${YELLOW}⚠ Binary $1 may not use jemalloc${NC}"
            echo "  Check Cargo.toml for jemalloc feature"
        fi
    fi
}

# Print current jemalloc configuration
print_config() {
    if [ -z "${MALLOC_CONF:-}" ]; then
        echo -e "${YELLOW}⚠ MALLOC_CONF not set (using jemalloc defaults)${NC}"
    else
        echo -e "${GREEN}Current MALLOC_CONF:${NC}"
        echo "  $MALLOC_CONF"
        echo ""
        echo "Parsed settings:"
        IFS=',' read -ra PARAMS <<< "$MALLOC_CONF"
        for param in "${PARAMS[@]}"; do
            echo "  - $param"
        done
    fi
}

# Run command with specified config
run_with_config() {
    local config_name=$1
    shift

    case "$config_name" in
        conservative)
            config_conservative
            ;;
        balanced)
            config_balanced
            ;;
        aggressive)
            config_aggressive
            ;;
        debug)
            config_debug
            ;;
        low-memory)
            config_low_memory
            ;;
        custom)
            local narenas=${1:-72}
            local tcache=${2:-true}
            local lg_tcache_max=${3:-15}
            shift 3 || true
            config_custom "$narenas" "$tcache" "$lg_tcache_max"
            ;;
        *)
            echo -e "${RED}✗ Unknown config: $config_name${NC}"
            echo "Available configs: conservative, balanced, aggressive, debug, low-memory, custom"
            return 1
            ;;
    esac

    echo ""
    echo -e "${GREEN}Running command: $@${NC}"
    exec "$@"
}

# Show help
show_help() {
    cat <<EOF
jemalloc Configuration Examples for MeTTaTron

Usage:
  source $0              # Load functions into shell
  $0 <config> <command>  # Run command with config

Available configurations:
  conservative   - Minimal changes (narenas:72)
  balanced       - Recommended for production (narenas:72 + tcache)
  aggressive     - Maximum concurrency (narenas:144 + all opts)
  debug          - Enable profiling and stats
  low-memory     - Minimize memory overhead
  custom         - Custom settings (usage: custom <narenas> <tcache> <lg_tcache_max>)

Functions (when sourced):
  config_conservative    - Apply conservative config
  config_balanced        - Apply balanced config
  config_aggressive      - Apply aggressive config
  config_debug           - Apply debug config
  config_low_memory      - Apply low-memory config
  config_custom          - Apply custom config
  verify_jemalloc [path] - Verify jemalloc installation
  print_config           - Print current MALLOC_CONF
  run_with_config        - Run command with config

Examples:
  # Run benchmarks with balanced config
  $0 balanced cargo bench --bench bulk_operations

  # Run tests with debug config
  $0 debug cargo test --release

  # Run binary with custom config (144 arenas, no tcache)
  $0 custom 144 false 0 ./target/release/mettatron

  # Use in shell (sourced)
  source $0
  config_balanced
  cargo bench

System Information:
  Detected CPUs: $(nproc)
  Recommended narenas: $(nproc) to $(($(nproc) * 2))
  Available RAM: $(free -h | awk '/^Mem:/ {print $2}')

Reference Documentation:
  See: docs/optimization/PATHMAP_JEMALLOC_ANALYSIS.md

EOF
}

# Main entry point
main() {
    if [ $# -eq 0 ]; then
        show_help
        exit 0
    fi

    case "$1" in
        -h|--help|help)
            show_help
            exit 0
            ;;
        verify)
            verify_jemalloc "${2:-}"
            exit 0
            ;;
        print)
            print_config
            exit 0
            ;;
        *)
            run_with_config "$@"
            ;;
    esac
}

# If script is executed (not sourced), run main
if [ "${BASH_SOURCE[0]}" = "${0}" ]; then
    main "$@"
fi
