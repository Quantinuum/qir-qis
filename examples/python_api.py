#!/usr/bin/env -S uv run --script
#
# /// script
# dependencies = [
#     "qir-qis",
#     "rich~=14.0",
# ]
#
# [tool.uv.sources]
# qir-qis = { path = "../" }
# ///
"""Example demonstrating the qir_qis Python API."""

from pathlib import Path

from rich import print as rprint

from qir_qis import get_entry_attributes, qir_ll_to_bc, qir_to_qis, validate_qir

# Read LLVM IR
with Path("tests/data/adaptive.ll").open() as f:
    llvm_ir = f.read()

# Convert LLVM IR text to bitcode
bc_bytes = qir_ll_to_bc(llvm_ir)

# Validate QIR
validate_qir(bc_bytes)

# Get entry point metadata
attributes = get_entry_attributes(bc_bytes)
rprint(f"Entry point attributes: {attributes}")

# Compile to QIS
qis_bytes = qir_to_qis(
    bc_bytes,
    opt_level=2,
    target="aarch64",
)

# Write output
with Path("output.qis.bc").open("wb") as f:
    f.write(qis_bytes)

print("Successfully compiled to output.qis.bc")
