# QIR-QIS

[![Crates.io](https://img.shields.io/crates/v/qir-qis)](https://crates.io/crates/qir-qis)
[![PyPI](https://img.shields.io/pypi/v/qir-qis)](https://pypi.org/project/qir-qis/)

A compiler that validates and translates [QIR (Quantum Intermediate Representation)](https://github.com/qir-alliance/qir-spec/tree/main/specification) to Quantinuum QIS (Quantum Instruction Set). This tool enables quantum programs written in QIR to run on Quantinuum's quantum computing systems.

## Features

- **QIR Validation**: Validates QIR bitcode for correctness and spec compliance
- **QIS Translation**: Compiles QIR to Quantinuum's native QIS instruction set
- **Python & Rust API**: Use as a Rust library or Python package
- **CLI Tool**: Command-line interface for quick compilation

See [qtm-qir-reference.md](https://github.com/quantinuum/qir-qis/blob/main/qtm-qir-reference.md) for details on supported QIR features and their mapping to Quantinuum QIS.

## Installation

### From Source (Rust)

**Requirements:**

- Rust >= 1.91.0
- LLVM 21

```sh
# Point llvm-sys to your LLVM 21 installation
export LLVM_SYS_211_PREFIX=/path/to/llvm21

cargo build --release
```

The compiled binary will be available at `target/release/qir-qis`.

### Python Package

**Requirements:**

- Python >= 3.10, < 3.15
- [uv](https://github.com/astral-sh/uv) (recommended) or pip

**Available pre-built wheels:**

- **Linux**: x86_64 (manylinux_2_28), aarch64 (manylinux_2_28)
- **macOS**: x86_64, arm64 (Apple Silicon)
- **Windows**: x86_64

All wheels support Python 3.10+ using the stable ABI (abi3).

```sh
# Using uv (recommended)
uv pip install qir-qis

# Using pip
pip install qir-qis
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

See [examples/python_api.py](https://github.com/quantinuum/qir-qis/blob/main/examples/python_api.py) for a complete working example.

```sh
uv run examples/python_api.py
```

For a more comprehensive example with quantum simulation, see [main.py](https://github.com/quantinuum/qir-qis/blob/main/main.py).

### Rust API

See [examples/rust_api.rs](https://github.com/quantinuum/qir-qis/blob/main/examples/rust_api.rs) for a complete working example.

```sh
cargo run --example rust_api
```

## Contributing

Contributions are welcome! Please read [CONTRIBUTING.md](https://github.com/quantinuum/qir-qis/blob/main/CONTRIBUTING.md) for:

- How to report issues and submit pull requests
- Coding standards and commit message format
- Development workflow and testing requirements

Development setup, test commands, and LLVM upgrade guidance live in [DEVELOPMENT.md](https://github.com/quantinuum/qir-qis/blob/main/DEVELOPMENT.md). Release notes are available in [CHANGELOG.md](https://github.com/quantinuum/qir-qis/blob/main/CHANGELOG.md).

## License

Apache-2.0

Copyright Quantinuum
