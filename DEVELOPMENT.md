# Development Notes

## Setup

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

## Project Structure

```text
qir-qis/
├── src/
│   ├── main.rs          # CLI entry point
│   ├── lib.rs           # Library and Python bindings
│   ├── convert.rs       # QIR to QIS conversion logic
│   ├── decompose.rs     # Gate decomposition
│   ├── opt.rs           # LLVM optimization passes
│   └── utils.rs         # Helper utilities
├── tests/
│   ├── data/            # Test QIR files
│   └── snaps/           # Snapshot test results
├── main.py              # Example Python usage with simulation
├── Cargo.toml           # Rust package configuration
├── pyproject.toml       # Python package configuration
└── Makefile             # Common development tasks
```

## Common Makefile Targets

| Command                    | Description                        |
|----------------------------|------------------------------------|
| `make test`                | Run all unit and integration tests |
| `make compile FILE=<path>` | Compile a single QIR file          |
| `make sim FILE=<path>`     | Compile and simulate a QIR file    |
| `make lint`                | Run code quality checks            |
| `make stubs`               | Regenerate Python type stubs       |
| `make allcompile`          | Compile all test files             |
| `make allsim`              | Simulate all test files            |

## Updating LLVM

LLVM version changes are mostly controlled from three places:

1. Workflow LLVM version variables:
   - `.github/workflows/CI.yml`
   - `.github/workflows/rust-release.yml`
   - `.github/workflows/wheels-release.yml`
2. The versioned `llvm-sys` prefix env var name passed through `.github/actions/setup-llvm/action.yml`.
3. Rust crate bindings in `Cargo.toml`:
   - `inkwell` feature (for example `llvm21-1`)
   - `llvm-sys` crate version (for example `211.0.0`)

For an LLVM major-version bump, update these in order:

1. Change `LLVM_VERSION` in the workflows to the desired full release, such as `22.0.0`.
2. Change the versioned env var name, such as `LLVM_SYS_211_PREFIX` to `LLVM_SYS_220_PREFIX`, in the workflow env blocks and any packaging config that still needs the explicit `llvm-sys` name.
3. Update `inkwell` and `llvm-sys` in `Cargo.toml`, then refresh `Cargo.lock`.
4. Check `pyproject.toml` for remaining version-specific wheel settings:
   - macOS Homebrew package name, such as `llvm@21`
   - versioned `LLVM_SYS_*` env var names on macOS and Windows
5. Check `README.md` for install examples that mention the old LLVM major.

The manylinux images intentionally install LLVM into `/opt/llvm`, so they should not need path changes for a version bump. Those paths are defined in:

- `.github/manylinux/Dockerfile.manylinux-2_34-x86_64`
- `.github/manylinux/Dockerfile.manylinux-2_34-aarch64`
- `.github/manylinux/llvm-config-wrapper.sh`

Recommended verification after a bump:

1. Run `cargo test --lib --all-features`.
2. Run `cargo run --example rust_api --all-features --profile test`.
3. Run `make lint`.
4. Run the CI matrix and wheel builds before cutting a release.
