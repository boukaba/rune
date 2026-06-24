#!/bin/sh
# Run each V8 benchmark script through Node.js.
# Usage: ./run_v8_baseline.sh [node_bin]
NODE="${1:-node}"

echo "=== V8 Baseline $(date '+%Y-%m-%d %H:%M') ==="
echo "Node: $("$NODE" --version 2>/dev/null || echo "unknown")"
echo ""

for src_file in crates/rune_bench/scripts/v8_*.js; do
    echo "--- $(basename "$src_file") ---"
    "$NODE" "$src_file" 2>&1
    echo ""
done
