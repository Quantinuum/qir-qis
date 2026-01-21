use qir_qis::{get_entry_attributes_core, qir_ll_to_bc_core, qir_to_qis_core, validate_qir_core};
use std::fs;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Read input
    let ll_text = fs::read_to_string("tests/data/adaptive.ll")?;

    // Convert LLVM IR text to bitcode
    let bc_bytes = qir_ll_to_bc_core(&ll_text)?;

    // Validate QIR
    validate_qir_core(&bc_bytes, None)?;

    // Get entry point metadata
    let attributes = get_entry_attributes_core(&bc_bytes)?;
    println!("Required qubits: {attributes:#?}");

    // Compile to QIS
    let qis_bytes = qir_to_qis_core(&bc_bytes, 2, "aarch64", None)?;

    // Write output
    fs::write("output.qis.bc", qis_bytes)?;

    println!("Successfully compiled to output.qis.bc");

    Ok(())
}
