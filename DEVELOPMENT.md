# Development Guide

## Testing with Nextest

We use [`cargo-nextest`](https://nexte.st/) as our test runner for faster parallel test execution and better output formatting.

### Installation

cargo-nextest is already installed globally. Verify with:

```bash
cargo nextest --version
```

### Basic Usage

Run all tests in the workspace:

```bash
cargo nextest run
```

Run tests for a specific package:

```bash
cargo nextest run -p holon
```

Run tests matching a pattern:

```bash
cargo nextest run feature_name
```

List all available tests without running them:

```bash
cargo nextest list
```

### Test Profiles

We have configured multiple profiles for different testing scenarios:

**`default`** - Standard development testing with pretty output and parallel execution (default)

```bash
cargo nextest run
```

**`quick`** - Fast sanity checks (60s timeout)

```bash
cargo nextest run --profile quick
```

**`ci`** - Strict CI/CD runs with JSON output, sequential execution, and retries

```bash
cargo nextest run --profile ci
```

**`dev`** - Development with verbose output and fail-fast mode (stops after first failure)

```bash
cargo nextest run --profile dev
```

### Configuration

Test runner configuration is in `Nextest.toml` in the workspace root. Key settings:

- **`test-threads`**: Number of parallel test threads (`auto` = all available CPUs)
- **`timeout`**: Individual test timeout in seconds (default: 300s)
- **`retries`**: Number of retries for flaky tests
- **`fail-fast`**: Stop after first failure
- **`output.format`**: `pretty` (default), `dot` (compact), or `json` (machine-readable)

### Combining with Code Coverage

Nextest works well with `cargo-llvm-cov` for coverage reporting:

```bash
cargo llvm-cov nextest --html --output-dir target/coverage-report
```

## Code Coverage

Code coverage helps identify dead code for elimination. We use `cargo-llvm-cov` to collect coverage data from tests.

### Prerequisites

```bash
cargo install cargo-llvm-cov
```

### Running Tests with Coverage

Run the property-based integration test with coverage:

```bash
cargo llvm-cov --test general_e2e_pbt -p holon-integration-tests --html --output-dir target/coverage-report
```

If tests fail but you still want the coverage report:

```bash
# Run tests (coverage data is collected even if tests fail)
cargo llvm-cov --test general_e2e_pbt -p holon-integration-tests 2>&1 || true

# Generate report from collected data
cargo llvm-cov report --html --output-dir target/coverage-report
```

### Viewing Coverage Reports

**HTML report** (interactive, best for exploration):
```bash
open target/coverage-report/html/index.html
```

**Text summary** (for quick overview):
```bash
cargo llvm-cov report --summary-only
```

**Holon packages only** (filter out dependencies):
```bash
cargo llvm-cov report --summary-only 2>&1 | grep -E "(^Filename|^----|^pkm/holon)" > target/coverage-report/holon-coverage-summary.txt
```

### Interpreting Results

The summary shows coverage by file with columns:
- **Regions/Cover**: Branch coverage
- **Functions/Executed**: Function coverage
- **Lines/Cover**: Line coverage (most useful for dead code detection)

**Dead code candidates**: Files with 0% line coverage are strong candidates for removal. Before removing, verify:
1. The code isn't used conditionally (feature flags, platform-specific)
2. No other tests exercise the code
3. The code isn't part of a planned feature

### Cleaning Coverage Data

```bash
cargo llvm-cov clean --workspace
```
