#!/usr/bin/env bash
set -euo pipefail

OUTPUT_DIR="${1:-target/api-docs}"

cargo doc --lib --no-deps --target-dir "$OUTPUT_DIR"

echo "API docs generated at $OUTPUT_DIR/doc/anchorkit/index.html"
