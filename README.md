# QIR-QIS

A compiler that validates and translates [QIR (Quantum Intermediate Representation)](https://github.com/qir-alliance/qir-spec/tree/main/specification) to Quantinuum QIS (Quantum Instruction Set). This tool enables quantum programs written in QIR to run on Quantinuum's quantum computing systems.

## Features

- **QIR Validation**: Validates QIR bitcode for correctness and spec compliance
- **QIS Translation**: Compiles QIR to Quantinuum's native QIS instruction set
- **Python & Rust API**: Use as a Rust library or Python package
- **CLI Tool**: Command-line interface for quick compilation

See [qtm-qir-reference.md](./qtm-qir-reference.md) for details on supported QIR features and their mapping to Quantinuum QIS.

## Installation

### From Source (Rust)

**Requirements:**

- Rust >= 1.91.0
- LLVM 14

```sh
cargo build --release
```

The compiled binary will be available at `target/release/qir-qis`.

### Python Package

**Requirements:**

- Python >= 3.10, < 3.15
- [uv](https://github.com/astral-sh/uv) (recommended) or pip

```sh
# Using uv (recommended)
uv pip install qir-qis

# Using pip
pip install qir-qis
```

For development installation:

```sh
uv sync
```

## Usage

### Command Line

Compile a QIR LLVM IR file to QIS bitcode:

```sh
# Basic usage
qir-qis input.ll

# With custom optimization level
qir-qis -O 3 input.ll

# Specify target architecture
qir-qis -t x86-64 input.ll

# Or using cargo
cargo run -- input.ll
```

This generates `input.qis.bc` containing the compiled QIS bitcode.

### Python API

See [examples/python_api.py](examples/python_api.py) for a complete working example.

```sh
uv run examples/python_api.py
```

For a more comprehensive example with quantum simulation, see [main.py](main.py).

### Rust API

See [examples/rust_api.rs](examples/rust_api.rs) for a complete working example.

```sh
cargo run --example rust_api
```

## Development

### Setting Up Development Environment

```sh
# Clone the repository
git clone https://github.com/quantinuum/qir-qis.git
cd qir-qis

# Install Rust dependencies and build
cargo build

# Install Python dependencies
uv sync
```

### Building

```sh
# Build Rust binary
cargo build --release

# Build Python package
uv run maturin build --release
```

### Testing

#### Running Tests

Tests require [cargo-nextest](https://nexte.st/docs/installation/pre-built-binaries/):

```sh
# Run all tests
make test

# Or directly with cargo
cargo nextest run --all-targets --all-features
```

#### Testing Individual Files

```sh
# Compile a single QIR file
make compile FILE=tests/data/adaptive.ll

# Compile all test files
make allcompile
```

#### Simulation Testing with Selene

Test the compiled QIS using [Selene quantum simulator](https://docs.quantinuum.com/selene/):

```sh
# Simulate a single file (runs 5 shots by default)
make sim FILE=tests/data/adaptive.ll

# Simulate all test files
make allsim
```

This will:

1. Compile the QIR to QIS
2. Run it on the Selene/Quest simulator
3. Display measurement results

### Code Quality

```sh
# Run linters
make lint

# This runs:
# - prek (pre-commit checks)
# - typos checker
# - cargo clippy
```

### Regenerating Python Stubs

After modifying the Python API:

```sh
make stubs
```

This updates [qir_qis.pyi](./qir_qis.pyi) with the latest type signatures.

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

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](CONTRIBUTING.md) for:

- How to report issues and submit pull requests
- Coding standards and commit message format
- Development workflow and testing requirements

**Quick checklist before submitting:**

- [ ] Tests pass: `make test`
- [ ] Linters pass: `make lint`
- [ ] Documentation updated
- [ ] [CHANGELOG.md](CHANGELOG.md) updated

## License

Apache-2.0

Copyright Quantinuum
