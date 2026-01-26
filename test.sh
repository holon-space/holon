#!/bin/bash
# Test runner script that ensures tests run sequentially
# This is necessary because tests use real network connections

set -e

echo "Running tests sequentially (--test-threads=1)..."
echo "This is required due to network resource usage."
echo ""

cargo test --tests -- --test-threads=1 "$@"
