# Development Notes

## Setup

The default development toolchain is tracked in `rust-toolchain.toml` and follows the latest stable Rust release. The minimum supported Rust version (MSRV) is tracked separately in `Cargo.toml` and wheel-build settings.

```sh
# Clone the repository
git clone https://github.com/quantinuum/qir-qis.git
cd qir-qis

# Install LLVM 21 (macOS/Homebrew example)
brew install llvm@21
export LLVM_SYS_211_PREFIX=/opt/homebrew/opt/llvm@21

# Install Rust dependencies and build
cargo build

# Install Python dependencies
uv sync
```

## Building

```sh
# Build Rust binary
LLVM_SYS_211_PREFIX=${LLVM_SYS_211_PREFIX:-/opt/homebrew/opt/llvm@21} \
cargo build --release

# Build Python package
LLVM_SYS_211_PREFIX=${LLVM_SYS_211_PREFIX:-/opt/homebrew/opt/llvm@21} \
uv run maturin build --release
```

## Testing

Tests require [cargo-nextest](https://nexte.st/docs/installation/pre-built-binaries/).

```sh
# Run all tests
LLVM_SYS_211_PREFIX=${LLVM_SYS_211_PREFIX:-/opt/homebrew/opt/llvm@21} \
make test

# Or directly with cargo
LLVM_SYS_211_PREFIX=${LLVM_SYS_211_PREFIX:-/opt/homebrew/opt/llvm@21} \
cargo nextest run --lib --all-features
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

- `prek` pre-commit checks
- `typos`
- `cargo clippy`

### Python Stubs

After modifying the Python API:

```sh
make stubs
```

This updates `qir_qis.pyi` with the latest type signatures.

## Updating LLVM

For an LLVM bump, update:

1. `LLVM_VERSION` in `.github/workflows/CI.yml`, `.github/workflows/rust-release.yml`, and `.github/workflows/wheels-release.yml` to the new full release, for example `22.1.0`.
2. The versioned `llvm-sys` env var name everywhere it appears, for example `LLVM_SYS_211_PREFIX` to `LLVM_SYS_220_PREFIX`.
3. `Cargo.toml`:
   - `inkwell` feature, for example `llvm21-1` to `llvm22-1`
   - `llvm-sys` crate version, for example `211.0.0` to `220.0.0`
4. `pyproject.toml` and `README.md` for any remaining version-specific install examples, especially `llvm@21` and `LLVM_SYS_*`.

The manylinux images already install to `/opt/llvm`, so they should not need path changes for a version bump.

After the bump, run:

1. `cargo test --lib --all-features`
2. `cargo run --example rust_api --all-features`
3. `make lint`
4. the CI matrix and wheel builds
