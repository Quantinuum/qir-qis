# Development Notes

## Setup

The default development toolchain is tracked in `rust-toolchain.toml` and follows the stable Rust release. The minimum supported Rust version (MSRV) is tracked separately in `Cargo.toml` and verified in CI. This project currently builds against LLVM 21.

```sh
# Clone the repository
git clone https://github.com/quantinuum/qir-qis.git
cd qir-qis

# Install LLVM 21 (macOS/Homebrew example)
brew install llvm@21
```

### Configure LLVM Environment

Set `LLVM_SYS_211_PREFIX` before running the build and test commands below. On macOS with Homebrew, `/path/to/llvm21` is typically `/opt/homebrew/opt/llvm@21`. On Linux, it is typically `/usr/lib/llvm-21`.

```sh
export LLVM_SYS_211_PREFIX=/path/to/llvm21
```

If you want this to persist across shells, add it to your shell startup file:

```sh
echo 'export LLVM_SYS_211_PREFIX=/path/to/llvm21' >> ~/.zshrc
source ~/.zshrc
```

Then install Rust dependencies and Python dependencies:

```sh
cargo build
uv sync
```

## Agent Guidance

If you are using an agentic coding tool, see [`AGENTS.md`](AGENTS.md). It captures the repo's standing expectations for preserving correctness guarantees while doing the main work of a PR, including when to add regression tests, property tests, fuzz coverage, or mutation-testing updates.

## Building

```sh
# Build Rust binary
cargo build --release

# Build Python package
uv run maturin build --release
```

## Testing

Tests require [cargo-nextest](https://nexte.st/docs/installation/pre-built-binaries/).

```sh
# Run all tests
make test

# Or directly with the same target as `make test`
cargo nextest run --all-targets --all-features
```

### QIR Fixtures

```sh
# Compile a single QIR file
make compile FILE=tests/data/adaptive.ll

# Compile all test files
make allcompile
```

### Simulation

Test the compiled QIS using [Selene quantum simulator](https://docs.quantinuum.com/selene/).

```sh
# Simulate a single file (runs 5 shots by default)
make sim FILE=tests/data/adaptive.ll

# Simulate all test files
make allsim
```

### Code Quality

```sh
# Run linters
make lint
```

`make lint` runs:

- `prek` checks, including formatting, `typos`, and Rust lint/doc hooks
- `ty` type checking

### Robustness Tooling

The repo also supports three complementary robustness checks:

```sh
# Mutation testing (install cargo-mutants first)
make mutants

# Run a bounded fuzz target locally (requires nightly + cargo-fuzz)
make fuzz FUZZ_TARGET=qir_to_qis FUZZ_RUN_ARGS=-max_total_time=30

# Run all current fuzz targets with short local smoke budgets
make fuzz-all
```

Recommended setup:

```sh
cargo install cargo-mutants cargo-fuzz
rustup toolchain install nightly
```

The property-based tests run as part of the normal Rust test suite, so `make test`
already exercises them.

### Python Stubs

After modifying the Python API:

```sh
make stubs
```

This updates `qir_qis.pyi` with the latest type signatures.

## Updating LLVM

LLVM upgrades are now coordinated with `hugrverse-env`, so there is not a stable
repo-local checklist beyond "update every place that pins the current ABI and
installation contract."

At minimum, review:

1. `Cargo.toml` for the `inkwell` feature and `llvm-sys` version.
2. `.github/workflows/*.yml` for the pinned `Quantinuum/hugrverse-env/install-hugrenv-action@...` version, and `hugrenv.lock` for the per-platform artifact hashes that go with it.
3. `.github/workflows/CI.yml`, `.github/workflows/robustness.yml`, and `.github/workflows/rust-release.yml` for the shared CI install flow.
4. `pyproject.toml` and `.github/workflows/wheels-release.yml` for wheel-build installation and environment wiring.
5. user-facing docs such as `README.md` and this file for LLVM version examples.

After changing the LLVM pin, run:

1. `make test`
2. `cargo run --example rust_api`
3. `make lint`
4. the CI matrix and wheel builds
